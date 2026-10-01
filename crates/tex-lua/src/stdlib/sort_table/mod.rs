use std::cmp::Ordering;

use crate::{LuaResult, LuaValue, lua_vm::LuaState};

/// table.sort(list [, comp]) - Sort table in place
///
/// 1. Extract elements to a Vec (raw access unless the table has a metatable)
/// 2. Default `<` on integers, strings or non-NaN numbers: Rust's sort (a total order)
/// 3. Otherwise: ltablib's quicksort, comparison for comparison
/// 4. Write the elements back
///
/// The buffer lives outside the Lua stack while comparators and metamethods
/// run Lua code (which may clear the table and collect garbage), so every
/// extracted element is also registered as a GC root until the sort ends.
pub fn table_sort(l: &mut LuaState) -> LuaResult<usize> {
    let roots_base = l.global_state().rust_roots.len();
    let result = table_sort_rooted(l);
    l.global_state_mut().rust_roots.truncate(roots_base);
    result
}

fn table_sort_rooted(l: &mut LuaState) -> LuaResult<usize> {
    let table_val = l
        .get_arg(1)
        .ok_or_else(|| crate::stdlib::lauxlib::typeerror(l, 1, "table"))?;
    let comp = l.get_arg(2);

    if !table_val.is_table() {
        return Err(crate::stdlib::lauxlib::typeerror(l, 1, "table"));
    }

    // Use obj_len to respect __len metamethod (like C Lua's aux_getn / luaL_len)
    let len = l.obj_len(&table_val)?;

    // C Lua: luaL_argcheck(L, n < INT_MAX, 1, "array too big");
    if len >= i32::MAX as i64 {
        return Err(l.error("bad argument #1 to 'sort' (array too big)".to_string()));
    }

    if len <= 1 {
        return Ok(0);
    }

    let comp_func = comp.unwrap_or_default();
    let has_comp = !comp_func.is_nil();
    if has_comp && !comp_func.is_function() {
        return Err(crate::stdlib::lauxlib::typeerror(l, 2, "function"));
    }

    let n = len as usize;

    // === Phase 1: Extract elements to buffer ===
    // Check if table has a metatable — if so, we must use table_geti/table_seti
    // to respect __index/__newindex. If not, raw access is safe and faster.
    let has_meta = table_val
        .as_table_mut()
        .map(|t| t.has_metatable())
        .unwrap_or(false);

    let mut buf: Vec<LuaValue> = Vec::with_capacity(n);
    if has_meta {
        for i in 1..=n {
            // Root each element before the next __index call can collect it.
            let val = l.table_geti(&table_val, i as i64)?;
            l.global_state_mut().rust_roots.push(val);
            buf.push(val);
        }
    } else {
        let table = table_val.as_table_mut().unwrap();
        for i in 1..=n {
            let val = table.raw_geti(i as i64).unwrap_or(LuaValue::nil());
            buf.push(val);
        }
        l.global_state_mut().rust_roots.extend_from_slice(&buf);
    }

    // Block yields during sort — sort is a non-continuable C call boundary
    l.nny += 1;

    // === Phase 2: Sort the buffer ===
    let result = sort_buffer(l, &mut buf, &comp_func, has_comp);

    // Restore yieldability before returning (even on error)
    l.nny -= 1;

    result?;

    // === Phase 3: Write back to table ===
    if has_meta {
        for (i, val) in buf.into_iter().enumerate() {
            l.table_seti(&table_val, (i + 1) as i64, val)?;
        }
    } else {
        let table = table_val.as_table_mut().unwrap();
        for (i, val) in buf.into_iter().enumerate() {
            table.raw_seti((i + 1) as i64, val);
        }
    }

    // GC write barrier
    if let Some(gc_ptr) = table_val.as_gc_ptr() {
        l.gc_barrier_back(gc_ptr);
    }

    Ok(0)
}

/// Sort the buffer using the best available algorithm.
fn sort_buffer(
    l: &mut LuaState,
    buf: &mut [LuaValue],
    comp_func: &LuaValue,
    has_comp: bool,
) -> LuaResult<()> {
    let n = buf.len();
    if n <= 1 {
        return Ok(());
    }

    // === Fast paths for the default `<` on numbers and strings ===
    // `sort_unstable_by` needs a total order (Rust >= 1.81 may panic otherwise), so
    // these paths are taken only when the comparison is exact and total: no NaN.
    if !has_comp {
        if buf.iter().all(|v| v.is_integer()) {
            buf.sort_unstable_by_key(|v| v.ivalue());
            return Ok(());
        }
        if buf.iter().all(|v| v.is_string()) {
            buf.sort_unstable_by(|a, b| {
                a.as_bytes().unwrap_or(&[]).cmp(b.as_bytes().unwrap_or(&[]))
            });
            return Ok(());
        }
        if buf
            .iter()
            .all(|v| v.is_integer() || (v.is_float() && !v.fltvalue().is_nan()))
        {
            buf.sort_unstable_by(num_cmp);
            return Ok(());
        }
    }

    // === General case: ltablib's quicksort ===
    auxsort(l, buf, 0, n - 1, 0, comp_func, has_comp)
}

/// Exact total order on non-NaN Lua numbers (mixed integers and floats).
fn num_cmp(a: &LuaValue, b: &LuaValue) -> Ordering {
    match (a.is_integer(), b.is_integer()) {
        (true, true) => a.ivalue().cmp(&b.ivalue()),
        (false, false) => a.fltvalue().partial_cmp(&b.fltvalue()).unwrap_or(Ordering::Equal),
        (true, false) => int_float_cmp(a.ivalue(), b.fltvalue()),
        (false, true) => int_float_cmp(b.ivalue(), a.fltvalue()).reverse(),
    }
}

/// Compare an integer with a non-NaN float without rounding the integer.
fn int_float_cmp(i: i64, f: f64) -> Ordering {
    // 2^63 is exactly representable; every i64 is below it and >= -2^63.
    if f >= 9223372036854775808.0 {
        return Ordering::Less;
    }
    if f < -9223372036854775808.0 {
        return Ordering::Greater;
    }
    let fl = f.floor();
    match i.cmp(&(fl as i64)) {
        Ordering::Equal if f > fl => Ordering::Less,
        o => o,
    }
}

// ============================================================
// Port of ltablib.c's quicksort (auxsort/partition/choosePivot).
// The comparison sequence matches C Lua, so comparators that are not strict
// weak orders behave the same way: either some permutation or the error
// "invalid order function for sorting". No comparator can cause a panic:
// every index stays within [lo, up].
// ============================================================

/// Compare two values: returns Ok(true) if a < b.
#[inline]
fn sort_compare(
    l: &mut LuaState,
    a: LuaValue,
    b: LuaValue,
    comp_func: &LuaValue,
    has_comp: bool,
) -> LuaResult<bool> {
    if has_comp {
        l.call_compare(*comp_func, a, b)
    } else {
        l.obj_lt(&a, &b)
    }
}

/// Partitions above this size choose a randomized pivot (C: RANLIMIT).
const RANLIMIT: usize = 100;

/// C: choosePivot. `rnd` is 0 until an unbalanced partition is seen.
fn choose_pivot(lo: usize, up: usize, rnd: usize) -> usize {
    let r4 = (up - lo) / 4;
    rnd % (r4 * 2) + (lo + r4)
}

/// C: l_randomizePivot (clock() + time() mixed). Any value works; it only
/// defends against adversarial inputs.
fn randomize_pivot() -> usize {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as usize)
        .unwrap_or(0);
    nanos ^ (nanos >> 17)
}

/// C: partition. Precondition: a[lo] <= P == a[up - 1] <= a[up].
fn partition(
    l: &mut LuaState,
    buf: &mut [LuaValue],
    lo: usize,
    up: usize,
    comp_func: &LuaValue,
    has_comp: bool,
) -> LuaResult<usize> {
    let mut i = lo;
    let mut j = up - 1;
    let pivot = buf[up - 1];
    loop {
        // next loop: repeat ++i while a[i] < P
        loop {
            i += 1;
            if !sort_compare(l, buf[i], pivot, comp_func, has_comp)? {
                break;
            }
            if i == up - 1 {
                return Err(l.error("invalid order function for sorting".to_string()));
            }
        }
        // after the loop, a[i] >= P and a[lo .. i - 1] < P
        // next loop: repeat --j while P < a[j]
        loop {
            j -= 1;
            if !sort_compare(l, pivot, buf[j], comp_func, has_comp)? {
                break;
            }
            // j < i but a[j] > P: also, j reaching lo means a[lo] > P
            if j < i {
                return Err(l.error("invalid order function for sorting".to_string()));
            }
        }
        // after the loop, a[j] <= P and a[j + 1 .. up] >= P
        if j < i {
            // no elements to be exchanged: swap pivot (a[up - 1]) with a[i]
            buf.swap(up - 1, i);
            return Ok(i);
        }
        buf.swap(i, j);
    }
}

/// C: auxsort.
fn auxsort(
    l: &mut LuaState,
    buf: &mut [LuaValue],
    mut lo: usize,
    mut up: usize,
    mut rnd: usize,
    comp_func: &LuaValue,
    has_comp: bool,
) -> LuaResult<()> {
    while lo < up {
        // sort elements 'lo', 'p', and 'up'
        if sort_compare(l, buf[up], buf[lo], comp_func, has_comp)? {
            buf.swap(lo, up);
        }
        if up - lo == 1 {
            break; // only 2 elements
        }
        let p = if up - lo < RANLIMIT || rnd == 0 {
            (lo + up) / 2
        } else {
            choose_pivot(lo, up, rnd)
        };
        if sort_compare(l, buf[p], buf[lo], comp_func, has_comp)? {
            buf.swap(p, lo);
        } else if sort_compare(l, buf[up], buf[p], comp_func, has_comp)? {
            buf.swap(p, up);
        }
        if up - lo == 2 {
            break; // only 3 elements
        }
        // swap pivot (a[p]) with a[up - 1]
        buf.swap(p, up - 1);
        let p = partition(l, buf, lo, up, comp_func, has_comp)?;
        // a[lo .. p - 1] <= a[p] == P <= a[p + 1 .. up]
        let n;
        if p - lo < up - p {
            auxsort(l, buf, lo, p - 1, rnd, comp_func, has_comp)?;
            n = p - lo;
            lo = p + 1;
        } else {
            auxsort(l, buf, p + 1, up, rnd, comp_func, has_comp)?;
            n = up - p;
            up = p - 1;
        }
        if (up - lo) / 128 > n {
            // partition too imbalanced: try a random pivot
            rnd = randomize_pivot();
        }
    }
    Ok(())
}
