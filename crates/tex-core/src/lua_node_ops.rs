//! List operations of the LuaTeX node library on the node store: the
//! algorithms behind `node.remove`, `node.insert_before`, `node.slide`,
//! `node.first_glyph`, `node.effective_glue`, `node.flatten_discretionaries`
//! and friends (lnodelib.c).

use crate::lua_node::*;

/// glyph slots (see `lua_node_conv::sl`)
const CHAR: usize = 0;
const FONT: usize = 1;
const COMP: usize = 6;
const EXPAN: usize = 12;

impl NodeStore {
    /// `set_t_to_prev`: the node before `current` found by walking from
    /// `head`, repairing prev links on the way; 0 when `current` is not in
    /// the list.
    fn find_prev(&mut self, head: u32, current: u32) -> u32 {
        let mut t = head;
        while t != 0 && self.nodes[t as usize].next != current {
            let nx = self.nodes[t as usize].next;
            if nx != 0 {
                self.nodes[nx as usize].prev = t;
            }
            t = nx;
        }
        t
    }

    /// `node.remove`: returns `(head, current)` after unlinking `current`
    /// (the node following it is the new current).
    pub fn remove(&mut self, mut head: u32, mut current: u32, direct: bool) -> Result<(u32, u32), String> {
        if head == 0 {
            return Ok((0, 0));
        }
        if current == 0 {
            return Ok((head, 0));
        }
        if head == current {
            let (p, n) = (self.nodes[current as usize].prev, self.nodes[current as usize].next);
            if p != 0 {
                self.nodes[p as usize].next = n;
            }
            if n != 0 {
                self.nodes[n as usize].prev = p;
            }
            head = n;
            current = n;
        } else {
            let mut t = self.nodes[current as usize].prev;
            if t == 0 || self.nodes[t as usize].next != current {
                t = self.find_prev(head, current);
                if t == 0 {
                    return Err(format!(
                        "Attempt to node.{}remove() a non-existing node",
                        if direct { "direct." } else { "" }
                    ));
                }
            }
            let n = self.nodes[current as usize].next;
            self.nodes[t as usize].next = n;
            if n != 0 {
                self.nodes[n as usize].prev = t;
            }
            current = n;
        }
        Ok((head, current))
    }

    /// `node.insert_before(head, current, n)`: `(head, n)`.
    pub fn insert_before(&mut self, head: u32, mut current: u32, n: u32) -> Result<(u32, u32), String> {
        if head == 0 {
            self.nodes[n as usize].next = 0;
            self.nodes[n as usize].prev = 0;
            return Ok((n, n));
        }
        if current == 0 {
            current = self.tail_of(head);
        }
        if head != current {
            let mut t = self.nodes[current as usize].prev;
            if t == 0 || self.nodes[t as usize].next != current {
                t = self.find_prev(head, current);
                if t == 0 {
                    return Err("Attempt to node.insert_before() a non-existing node".to_string());
                }
            }
            self.couple(t, n);
        }
        self.couple(n, current);
        Ok((if head == current { n } else { head }, n))
    }

    /// `node.insert_after(head, current, n)`: `(head, n)`.
    pub fn insert_after(&mut self, head: u32, mut current: u32, n: u32) -> (u32, u32) {
        if head == 0 {
            self.nodes[n as usize].next = 0;
            self.nodes[n as usize].prev = 0;
            return (n, n);
        }
        if current == 0 {
            current = self.tail_of(head);
        }
        let nx = self.nodes[current as usize].next;
        if nx != 0 {
            self.couple(n, nx);
        }
        self.couple(current, n);
        (head, n)
    }

    /// `node.slide`: the last node of the list, repairing prev links.
    pub fn slide(&mut self, mut n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        while self.nodes[n as usize].next != 0 {
            let nx = self.nodes[n as usize].next;
            self.nodes[nx as usize].prev = n;
            n = nx;
        }
        n
    }

    /// `node.end_of_math`
    pub fn end_of_math(&self, mut n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        loop {
            let node = &self.nodes[n as usize];
            if node.id == MATH && node.subtype == 1 {
                return n;
            }
            if node.next == 0 {
                return 0;
            }
            n = node.next;
        }
    }

    /// `do_lua_nodelib_count`: nodes from `first` up to (excluding) `stop`
    /// with type `id` (every node when `id < 0`).
    pub fn count(&self, id: i64, mut first: u32, stop: u32) -> i64 {
        let mut c = 0;
        while first != stop && first != 0 {
            if id < 0 || i64::from(self.nodes[first as usize].id) == id {
                c += 1;
            }
            first = self.nodes[first as usize].next;
        }
        c
    }

    /// `node.first_glyph(h[, t])`: the first plain character in `h..t`.
    pub fn first_glyph(&self, mut h: u32, t: u32) -> u32 {
        while h != 0 {
            let n = &self.nodes[h as usize];
            if n.id == GLYPH && n.subtype & (GLYPH_LIGATURE | GLYPH_GHOST) == 0 && n.subtype & GLYPH_CHARACTER != 0
            {
                return h;
            }
            if h == t {
                return 0;
            }
            h = n.next;
        }
        0
    }

    pub fn has_glyph(&self, mut h: u32) -> u32 {
        while h != 0 {
            let n = &self.nodes[h as usize];
            if n.id == GLYPH || n.id == DISC {
                return h;
            }
            h = n.next;
        }
        0
    }

    /// `flatten_discretionaries`: `(head, count)`.
    pub fn flatten_discretionaries(&mut self, mut head: u32) -> (u32, i64) {
        let mut current = head;
        let mut c = 0;
        while current != 0 {
            let next = self.nodes[current as usize].next;
            if self.nodes[current as usize].id == DISC {
                c += 1;
                let h = self.nodes[current as usize].f[2] as u32;
                let prev = self.nodes[current as usize].prev;
                if h != 0 {
                    let t = self.tail_of(h);
                    if next != 0 {
                        self.couple(t, next);
                    } else {
                        self.nodes[t as usize].next = 0;
                    }
                    if current == head {
                        head = h;
                    } else if prev != 0 {
                        self.couple(prev, h);
                    }
                    self.nodes[current as usize].f[2] = 0;
                } else if current == head {
                    head = next;
                } else if prev != 0 {
                    self.couple(prev, next);
                }
                self.flush_node(current);
            }
            current = next;
        }
        (head, c)
    }

    /// `protect_glyph_node` / `unprotect_glyph_node` over a glyph or the
    /// glyphs inside a discretionary.
    pub fn protect(&mut self, n: u32, on: bool) {
        let apply = |s: &mut NodeStore, g: u32| {
            let sub = &mut s.nodes[g as usize].subtype;
            *sub = if on { *sub | 0xFF00 } else { *sub & 0x00FF };
        };
        match self.nodes[n as usize].id {
            GLYPH => apply(self, n),
            DISC => {
                for slot in [2usize, 0, 1] {
                    let mut h = self.nodes[n as usize].f[slot] as u32;
                    while h != 0 {
                        if self.nodes[h as usize].id == GLYPH {
                            apply(self, h);
                        }
                        h = self.nodes[h as usize].next;
                    }
                }
            }
            _ => {}
        }
    }

    /// `node.uses_font`
    pub fn uses_font(&self, n: u32, font: i64) -> bool {
        let node = &self.nodes[n as usize];
        match node.id {
            GLYPH => i64::from(node.f[FONT]) == font,
            DISC => [0usize, 1, 2].iter().any(|&slot| {
                let mut p = node.f[slot] as u32;
                while p != 0 {
                    let q = &self.nodes[p as usize];
                    if q.id == GLYPH && i64::from(q.f[FONT]) == font {
                        return true;
                    }
                    p = q.next;
                }
                false
            }),
            _ => false,
        }
    }

    /// `node.protrusion_skippable` (linebreak.h `cp_skipable`)
    pub fn cp_skippable(&self, n: u32) -> bool {
        let node = &self.nodes[n as usize];
        match node.id {
            GLUE => node.f[1] == 0 && node.f[2] == 0 && node.f[3] == 0,
            PENALTY | DIR | LOCAL_PAR | INS | MARK | ADJUST | BOUNDARY | WHATSIT => true,
            DISC => node.f[0] == 0 && node.f[1] == 0 && node.f[2] == 0,
            KERN => node.f[0] == 0 || node.subtype == 0,
            RULE => node.f[0] == 0 && node.f[2] == 0 && node.f[1] == 0,
            MATH => node.f[0] == 0 || (node.f[1] == 0 && node.f[2] == 0 && node.f[3] == 0),
            HLIST => node.f[8] == 0 && node.f[0] == 0 && node.f[2] == 0 && node.f[1] == 0,
            _ => false,
        }
    }

    /// `node.effective_glue`
    pub fn effective_glue(&self, glue: u32, parent: u32) -> Option<f64> {
        let g = &self.nodes[glue as usize];
        if g.id != GLUE {
            return None;
        }
        let mut w = f64::from(g.f[1]);
        if parent != 0 {
            let p = &self.nodes[parent as usize];
            if p.id == HLIST || p.id == VLIST {
                // luatex sign: 1 stretching, 2 shrinking; orders as stored
                match p.f[6] {
                    1 if g.f[4] == p.f[5] => w += f64::from(g.f[2]) * p.fl,
                    2 if g.f[5] == p.f[5] => w -= f64::from(g.f[3]) * p.fl,
                    _ => {}
                }
            }
        }
        Some(w)
    }

    /// The nodes of a list in order.
    pub fn list_nodes(&self, mut n: u32) -> Vec<u32> {
        let mut v = Vec::new();
        while n != 0 {
            v.push(n);
            n = self.nodes[n as usize].next;
        }
        v
    }
}
