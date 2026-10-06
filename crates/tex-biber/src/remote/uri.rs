//! URI::_generic::abs (URI 5.27). Unlike WHATWG Url::join, this preserves
//! absolute paths' dot segments and unresolved leading parents.
#[derive(Clone, Copy)]
pub(super) struct Parts<'a> {
    pub(super) scheme: Option<&'a str>,
    pub(super) authority: Option<&'a str>,
    pub(super) path: &'a str,
    pub(super) query: Option<&'a str>,
    pub(super) fragment: Option<&'a str>,
}

pub(super) fn parts(uri: &str) -> Parts<'_> {
    let (uri, fragment) = uri.split_once('#').map_or((uri, None), |(s, f)| (s, Some(f)));
    let (scheme, rest) = uri.split_once(':').filter(|(scheme, _)| {
        scheme.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
            && scheme.bytes().all(|c| c.is_ascii_alphanumeric() || b"+.-".contains(&c))
    }).map_or((None, uri), |(scheme, rest)| (Some(scheme), rest));
    let (authority, rest) = if let Some(rest) = rest.strip_prefix("//") {
        let end = rest.find(['/', '?']).unwrap_or(rest.len());
        (Some(&rest[..end]), &rest[end..])
    } else { (None, rest) };
    let (path, query) = rest.split_once('?').map_or((rest, None), |(path, query)| (path, Some(query)));
    Parts { scheme, authority, path, query, fragment }
}

pub(super) fn absolute(reference: &str, base: &str, allow_relative_scheme: bool) -> String {
    let reference_parts = parts(reference);
    let base_parts = parts(base);
    if let Some(scheme) = reference_parts.scheme {
        if !allow_relative_scheme || !base_parts.scheme.is_some_and(|base| scheme.eq_ignore_ascii_case(base)) { return reference.to_owned(); }
    }
    let scheme = base_parts.scheme.unwrap_or("");
    if reference_parts.authority.is_some() { return assemble(scheme, reference_parts.authority, reference_parts.path, reference_parts.query, reference_parts.fragment); }
    let authority = base_parts.authority;
    if reference_parts.path.starts_with('/') { return assemble(scheme, authority, reference_parts.path, reference_parts.query, reference_parts.fragment); }
    if reference_parts.path.is_empty() {
        return assemble(scheme, authority, base_parts.path, reference_parts.query.or(base_parts.query), reference_parts.fragment.or(base_parts.fragment));
    }
    let prefix = base_parts.path.rfind('/').map_or("", |last| &base_parts.path[..last + 1]);
    let combined = format!("{prefix}{}", reference_parts.path);
    let path = combined.strip_prefix('/').unwrap_or(&combined);
    let mut segments = Vec::new();
    let mut input = path.split('/').peekable();
    while let Some(segment) = input.next() {
        let last = input.peek().is_none();
        if segment == "." {
            if last { segments.push(""); }
        } else if segment == ".." && segments.last().is_some_and(|&previous| previous != "..") {
            segments.pop();
            if last && !segments.is_empty() { segments.push(""); }
        } else { segments.push(segment); }
    }
    let path = format!("/{}", segments.join("/"));
    assemble(scheme, authority, &path, reference_parts.query, reference_parts.fragment)
}

fn assemble(scheme: &str, authority: Option<&str>, path: &str, query: Option<&str>, fragment: Option<&str>) -> String {
    let mut result = String::new();
    result.extend(scheme.chars().map(|c| c.to_ascii_lowercase())); result.push(':');
    if let Some(authority) = authority { result.push_str("//"); result.push_str(authority); }
    result.push_str(path);
    if let Some(query) = query { result.push('?'); result.push_str(query); }
    if let Some(fragment) = fragment { result.push('#'); result.push_str(fragment); }
    result
}
