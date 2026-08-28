//! Native, lexical path utilities for Den scripts.

use std::env;

use rquickjs::{
    Coerced, Ctx, Exception, FromJs as _, Function, IntoJs as _, Object, Result, Value,
    function::Rest,
};

const EXPORTS: [&str; 17] = [
    "basename",
    "default",
    "delimiter",
    "dirname",
    "extname",
    "format",
    "isAbsolute",
    "join",
    "matchesGlob",
    "normalize",
    "parse",
    "posix",
    "relative",
    "resolve",
    "sep",
    "toNamespacedPath",
    "windows",
];

#[derive(Clone, Copy)]
enum Style {
    Posix,
    Windows,
}

#[derive(Clone, Copy)]
enum Operation {
    Resolve,
    Normalize,
    IsAbsolute,
    Join,
    Relative,
    ToNamespacedPath,
    Dirname,
    Basename,
    Extname,
    Format,
    Parse,
    MatchesGlob,
}

impl Operation {
    const fn length(self) -> usize {
        match self {
            Self::Resolve | Self::Join => 0,
            Self::Normalize
            | Self::IsAbsolute
            | Self::ToNamespacedPath
            | Self::Dirname
            | Self::Extname
            | Self::Format
            | Self::Parse => 1,
            Self::Relative | Self::Basename | Self::MatchesGlob => 2,
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Parts {
    root: String,
    dir:  String,
    base: String,
    ext:  String,
    name: String,
}

#[derive(Debug, Default)]
struct WindowsRoot {
    device:   String,
    end:      usize,
    absolute: bool,
    unc:      bool,
}

const fn windows_separator(byte: u8) -> bool { byte == b'/' || byte == b'\\' }

const fn drive_letter(byte: u8) -> bool { byte.is_ascii_alphabetic() }

fn windows_root(path: &str) -> WindowsRoot {
    let bytes = path.as_bytes();
    let Some(first) = bytes.first() else {
        return WindowsRoot::default();
    };
    if windows_separator(*first) {
        let mut root = WindowsRoot {
            end: 1,
            absolute: true,
            ..WindowsRoot::default()
        };
        if !bytes.get(1).is_some_and(|byte| windows_separator(*byte)) {
            return root;
        }
        let mut index = 2;
        let server_start = index;
        while bytes
            .get(index)
            .is_some_and(|byte| !windows_separator(*byte))
        {
            index += 1;
        }
        if index == server_start || index == bytes.len() {
            return root;
        }
        let server = path.get(server_start..index).unwrap_or_default();
        while bytes
            .get(index)
            .is_some_and(|byte| windows_separator(*byte))
        {
            index += 1;
        }
        let share_start = index;
        while bytes
            .get(index)
            .is_some_and(|byte| !windows_separator(*byte))
        {
            index += 1;
        }
        if index == share_start {
            return root;
        }
        if matches!(server, "." | "?") {
            root.device = format!(r"\\{server}");
            root.end = 4.min(bytes.len());
        } else {
            root.device = format!(
                r"\\{server}\{}",
                path.get(share_start..index).unwrap_or_default()
            );
            root.end = index;
            root.unc = true;
        }
        return root;
    }
    if bytes.first().is_some_and(|byte| drive_letter(*byte)) && bytes.get(1) == Some(&b':') {
        let absolute = bytes.get(2).is_some_and(|byte| windows_separator(*byte));
        return WindowsRoot {
            device: path.get(..2).unwrap_or_default().to_owned(),
            end: if absolute { 3 } else { 2 },
            absolute,
            unc: false,
        };
    }
    WindowsRoot::default()
}

fn normalize_segments(path: &str, absolute: bool, style: Style) -> String {
    let separator = |character| {
        match style {
            Style::Posix => character == '/',
            Style::Windows => matches!(character, '/' | '\\'),
        }
    };
    let mut segments = Vec::new();
    for segment in path.split(separator) {
        match segment {
            "" | "." => {}
            ".." => {
                if segments.last().is_some_and(|last| *last != "..") {
                    segments.pop();
                } else if !absolute {
                    segments.push(segment);
                }
            }
            _ => segments.push(segment),
        }
    }
    segments.join(match style {
        Style::Posix => "/",
        Style::Windows => "\\",
    })
}

fn cwd() -> String {
    env::current_dir()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn posix_normalize(path: &str) -> String {
    if path.is_empty() {
        return ".".to_owned();
    }
    let absolute = path.starts_with('/');
    let trailing = path.ends_with('/');
    let mut tail = normalize_segments(path, absolute, Style::Posix);
    if tail.is_empty() {
        return if absolute {
            "/".to_owned()
        } else if trailing {
            "./".to_owned()
        } else {
            ".".to_owned()
        };
    }
    if trailing {
        tail.push('/');
    }
    if absolute { format!("/{tail}") } else { tail }
}

fn windows_normalize(path: &str) -> String {
    if path.is_empty() {
        return ".".to_owned();
    }
    if path.len() == 1 {
        return if path == "/" {
            "\\".to_owned()
        } else {
            path.to_owned()
        };
    }
    let root = windows_root(path);
    if root.unc && root.end == path.len() {
        return format!("{}\\", root.device);
    }
    let trailing = path
        .as_bytes()
        .last()
        .is_some_and(|byte| windows_separator(*byte));
    let mut tail = normalize_segments(
        path.get(root.end..).unwrap_or_default(),
        root.absolute,
        Style::Windows,
    );
    if tail.is_empty() && !root.absolute {
        tail.push('.');
    }
    if !tail.is_empty() && trailing {
        tail.push('\\');
    }
    if root.device.is_empty() {
        if root.absolute {
            format!(r"\{tail}")
        } else {
            tail
        }
    } else if root.absolute {
        format!(r"{}\{tail}", root.device)
    } else {
        format!("{}{tail}", root.device)
    }
}

fn posix_resolve(paths: &[String]) -> String {
    if paths.is_empty()
        || (paths.len() == 1
            && paths
                .first()
                .is_some_and(|path| matches!(path.as_str(), "" | ".")))
    {
        let cwd = cwd();
        if cwd.starts_with('/') {
            return cwd;
        }
    }
    let mut resolved = String::new();
    let mut absolute = false;
    for path in paths.iter().rev() {
        if path.is_empty() {
            continue;
        }
        resolved = format!("{path}/{resolved}");
        absolute = path.starts_with('/');
        if absolute {
            break;
        }
    }
    if !absolute {
        let cwd = cwd();
        resolved = format!("{cwd}/{resolved}");
        absolute = cwd.starts_with('/');
    }
    let tail = normalize_segments(&resolved, absolute, Style::Posix);
    if absolute {
        format!("/{tail}")
    } else if tail.is_empty() {
        ".".to_owned()
    } else {
        tail
    }
}

fn windows_resolve(paths: &[String]) -> String {
    let mut device = String::new();
    let mut tail = String::new();
    let mut absolute = false;
    let mut index = paths.len() as isize - 1;
    while index >= -1 {
        let path = if index >= 0 {
            let argument_index = index as usize;
            let Some(path) = paths.get(argument_index).cloned() else {
                index -= 1;
                continue;
            };
            if path.is_empty() {
                index -= 1;
                continue;
            }
            path
        } else if device.is_empty() {
            cwd()
        } else {
            let drive_dir = env::var(format!("={device}")).unwrap_or_else(|_| cwd());
            let root = windows_root(&drive_dir);
            if root.device.eq_ignore_ascii_case(&device) {
                drive_dir
            } else {
                format!(r"{device}\")
            }
        };
        let root = windows_root(&path);
        if !root.device.is_empty() {
            if device.is_empty() {
                device.clone_from(&root.device);
            } else if !root.device.eq_ignore_ascii_case(&device) {
                index -= 1;
                continue;
            }
        }
        if absolute {
            if !device.is_empty() {
                break;
            }
        } else {
            tail = format!(r"{}\{tail}", path.get(root.end..).unwrap_or_default());
            absolute = root.absolute;
            if root.absolute && !device.is_empty() {
                break;
            }
        }
        index -= 1;
    }
    let tail = normalize_segments(&tail, absolute, Style::Windows);
    let resolved = if absolute {
        format!(r"{device}\{tail}")
    } else {
        format!("{device}{tail}")
    };
    if resolved.is_empty() {
        ".".to_owned()
    } else {
        resolved
    }
}

fn resolve(paths: &[String], style: Style) -> String {
    match style {
        Style::Posix => posix_resolve(paths),
        Style::Windows => windows_resolve(paths),
    }
}

fn normalize(path: &str, style: Style) -> String {
    match style {
        Style::Posix => posix_normalize(path),
        Style::Windows => windows_normalize(path),
    }
}

fn is_absolute(path: &str, style: Style) -> bool {
    match style {
        Style::Posix => path.starts_with('/'),
        Style::Windows => {
            let bytes = path.as_bytes();
            bytes.first().is_some_and(|byte| windows_separator(*byte))
                || (bytes.first().is_some_and(|byte| drive_letter(*byte))
                    && bytes.get(1) == Some(&b':')
                    && bytes.get(2).is_some_and(|byte| windows_separator(*byte)))
        }
    }
}

fn join(paths: &[String], style: Style) -> String {
    let nonempty: Vec<&str> = paths
        .iter()
        .map(String::as_str)
        .filter(|path| !path.is_empty())
        .collect();
    if nonempty.is_empty() {
        return ".".to_owned();
    }
    match style {
        Style::Posix => posix_normalize(&nonempty.join("/")),
        Style::Windows => windows_normalize(&nonempty.join("\\")),
    }
}

fn relative(from: &str, to: &str, style: Style) -> String {
    if from == to {
        return String::new();
    }
    let (from, to, separator) = match style {
        Style::Posix => {
            (
                posix_resolve(&[from.to_owned()]),
                posix_resolve(&[to.to_owned()]),
                "/",
            )
        }
        Style::Windows => {
            (
                windows_resolve(&[from.to_owned()]),
                windows_resolve(&[to.to_owned()]),
                "\\",
            )
        }
    };
    let equal = match style {
        Style::Posix => from == to,
        Style::Windows => from.to_lowercase() == to.to_lowercase(),
    };
    if equal {
        return String::new();
    }
    if matches!(style, Style::Windows) {
        let from_root = windows_root(&from);
        let to_root = windows_root(&to);
        if !from_root.device.eq_ignore_ascii_case(&to_root.device) {
            return to;
        }
    }
    let split = |path: &str| {
        path.split(if matches!(style, Style::Posix) {
            '/'
        } else {
            '\\'
        })
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>()
    };
    let from_parts = split(&from);
    let to_parts = split(&to);
    let mut common = 0;
    while let (Some(from_part), Some(to_part)) = (from_parts.get(common), to_parts.get(common)) {
        let same = match style {
            Style::Posix => from_part == to_part,
            Style::Windows => from_part.to_lowercase() == to_part.to_lowercase(),
        };
        if !same {
            break;
        }
        common += 1;
    }
    let mut output = vec!["..".to_owned(); from_parts.len() - common];
    output.extend_from_slice(to_parts.get(common..).unwrap_or_default());
    output.join(separator)
}

fn dirname(path: &str, style: Style) -> String {
    if path.is_empty() {
        return ".".to_owned();
    }
    let bytes = path.as_bytes();
    let boundary = match style {
        Style::Posix => usize::from(path.starts_with('/')),
        Style::Windows => {
            let root = windows_root(path);
            if root.unc && root.end == path.len() {
                return path.to_owned();
            }
            if root.unc && root.end < path.len() {
                root.end + 1
            } else {
                root.end
            }
        }
    };
    let separator = |byte| {
        match style {
            Style::Posix => byte == b'/',
            Style::Windows => windows_separator(byte),
        }
    };
    let mut end = None;
    let mut trailing = true;
    for index in (boundary..bytes.len()).rev() {
        if bytes.get(index).is_some_and(|byte| separator(*byte)) {
            if !trailing {
                end = Some(index);
                break;
            }
        } else {
            trailing = false;
        }
    }
    end.map_or_else(
        || {
            if boundary > 0 {
                path.get(..boundary).unwrap_or_default().to_owned()
            } else {
                ".".to_owned()
            }
        },
        |end| {
            if matches!(style, Style::Posix) && boundary == 1 && end == 1 {
                "//".to_owned()
            } else {
                path.get(..end).unwrap_or_default().to_owned()
            }
        },
    )
}

fn basename(path: &str, suffix: Option<&str>, style: Style) -> String {
    let separator = |byte| {
        match style {
            Style::Posix => byte == b'/',
            Style::Windows => windows_separator(byte),
        }
    };
    let mut start = if matches!(style, Style::Windows)
        && path
            .as_bytes()
            .first()
            .is_some_and(|byte| drive_letter(*byte))
        && path.as_bytes().get(1) == Some(&b':')
    {
        2
    } else {
        0
    };
    let bytes = path.as_bytes();
    let mut end = bytes.len();
    while end > start && bytes.get(end - 1).is_some_and(|byte| separator(*byte)) {
        end -= 1;
    }
    if end == start {
        return String::new();
    }
    if let Some(index) = bytes
        .get(start..end)
        .unwrap_or_default()
        .iter()
        .rposition(|byte| separator(*byte))
    {
        start += index + 1;
    }
    let base = path.get(start..end).unwrap_or_default();
    suffix
        .and_then(|suffix| base.strip_suffix(suffix))
        .unwrap_or(base)
        .to_owned()
}

fn extname(path: &str, style: Style) -> String {
    let base = basename(path, None, style);
    if matches!(base.as_str(), "" | "." | "..") {
        return String::new();
    }
    match base.rfind('.') {
        Some(0) | None => String::new(),
        Some(index) => base.get(index..).unwrap_or_default().to_owned(),
    }
}

fn parse(path: &str, style: Style) -> Parts {
    if path.is_empty() {
        return Parts::default();
    }
    let (root, root_end) = match style {
        Style::Posix if path.starts_with('/') => ("/".to_owned(), 1),
        Style::Posix => (String::new(), 0),
        Style::Windows => {
            let root = windows_root(path);
            let end = if root.unc && root.end < path.len() {
                root.end + 1
            } else {
                root.end
            };
            (path.get(..end).unwrap_or_default().to_owned(), end)
        }
    };
    let separator = |byte| {
        match style {
            Style::Posix => byte == b'/',
            Style::Windows => windows_separator(byte),
        }
    };
    let bytes = path.as_bytes();
    let mut end = bytes.len();
    while end > root_end && bytes.get(end - 1).is_some_and(|byte| separator(*byte)) {
        end -= 1;
    }
    let start = bytes
        .get(root_end..end)
        .unwrap_or_default()
        .iter()
        .rposition(|byte| separator(*byte))
        .map_or(root_end, |index| root_end + index + 1);
    let base = path.get(start..end).unwrap_or_default().to_owned();
    let ext = extname(&base, style);
    let name = base
        .get(..base.len() - ext.len())
        .unwrap_or_default()
        .to_owned();
    let dir = if start > root_end {
        path.get(..start - 1).unwrap_or_default().to_owned()
    } else {
        root.clone()
    };
    Parts {
        root,
        dir,
        base,
        ext,
        name,
    }
}

fn expand_alternatives(pattern: &str) -> Vec<String> {
    fn expand(pattern: &str, open: char, close: char, marker: Option<char>) -> Option<Vec<String>> {
        let chars: Vec<(usize, char)> = pattern.char_indices().collect();
        for (position, &(start, character)) in chars.iter().enumerate() {
            let content_start = if marker == Some(character)
                && chars
                    .get(position + 1)
                    .is_some_and(|(_, next)| *next == open)
            {
                chars
                    .get(position + 1)
                    .map_or(start, |(index, _)| *index + open.len_utf8())
            } else if marker.is_none() && character == open {
                start + open.len_utf8()
            } else {
                continue;
            };
            let mut depth = 1;
            let mut separators = Vec::new();
            let mut end = None;
            for &(index, current) in chars
                .iter()
                .skip(position + if marker.is_some() { 2 } else { 1 })
            {
                if current == open {
                    depth += 1;
                } else if current == close {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(index);
                        break;
                    }
                } else if depth == 1 && (current == ',' || marker.is_some() && current == '|') {
                    separators.push(index);
                }
            }
            let end = end?;
            if separators.is_empty() {
                continue;
            }
            let prefix = pattern.get(..start).unwrap_or_default();
            let suffix = pattern.get(end + close.len_utf8()..).unwrap_or_default();
            let mut bounds = Vec::with_capacity(separators.len() + 2);
            bounds.push(content_start);
            bounds.extend(separators.iter().map(|index| index + 1));
            bounds.push(end + 1);
            let mut output = Vec::new();
            for pair in bounds.windows(2) {
                let Some((&alternative_start, &next_start)) = pair.first().zip(pair.get(1)) else {
                    continue;
                };
                output.push(format!(
                    "{prefix}{}{suffix}",
                    pattern
                        .get(alternative_start..next_start - 1)
                        .unwrap_or_default()
                ));
            }
            return Some(output);
        }
        None
    }

    let mut pending = vec![pattern.to_owned()];
    let mut output = Vec::new();
    while let Some(pattern) = pending.pop() {
        if let Some(expanded) =
            expand(&pattern, '{', '}', None).or_else(|| expand(&pattern, '(', ')', Some('@')))
        {
            pending.extend(expanded);
        } else {
            output.push(pattern);
        }
    }
    output
}

#[expect(
    clippy::indexing_slicing,
    reason = "glob DP indices are range-checked immediately before every access"
)]
fn glob_match_recursive(
    path: &[char], pattern: &[char], path_index: usize, pattern_index: usize,
    memo: &mut [Vec<Option<bool>>],
) -> bool {
    if let Some(cached) = memo[path_index][pattern_index] {
        return cached;
    }
    let result = if pattern_index == pattern.len() {
        path_index == path.len()
    } else if pattern[pattern_index] == '*' {
        let mut end = pattern_index;
        while end < pattern.len() && pattern[end] == '*' {
            end += 1;
        }
        let globstar = end - pattern_index >= 2
            && (pattern_index == 0 || pattern[pattern_index - 1] == '/')
            && (end == pattern.len() || pattern[end] == '/');
        if globstar {
            let skip = if end < pattern.len() && pattern[end] == '/' {
                end + 1
            } else {
                end
            };
            glob_match_recursive(path, pattern, path_index, skip, memo)
                || path_index < path.len()
                    && !(path[path_index] == '.'
                        && (path_index == 0 || path[path_index - 1] == '/'))
                    && glob_match_recursive(path, pattern, path_index + 1, pattern_index, memo)
        } else {
            glob_match_recursive(path, pattern, path_index, end, memo)
                || path_index < path.len()
                    && path[path_index] != '/'
                    && !(path[path_index] == '.'
                        && (path_index == 0 || path[path_index - 1] == '/'))
                    && glob_match_recursive(path, pattern, path_index + 1, pattern_index, memo)
        }
    } else if pattern[pattern_index] == '?' {
        path_index < path.len()
            && path[path_index] != '/'
            && !(path[path_index] == '.' && (path_index == 0 || path[path_index - 1] == '/'))
            && glob_match_recursive(path, pattern, path_index + 1, pattern_index + 1, memo)
    } else if pattern[pattern_index] == '[' {
        class_match(path, pattern, path_index, pattern_index).is_some_and(|(matched, next)| {
            matched && glob_match_recursive(path, pattern, path_index + 1, next, memo)
        })
    } else {
        path_index < path.len()
            && path[path_index] == pattern[pattern_index]
            && glob_match_recursive(path, pattern, path_index + 1, pattern_index + 1, memo)
    };
    memo[path_index][pattern_index] = Some(result);
    result
}

#[expect(
    clippy::indexing_slicing,
    reason = "character-class indices are checked against the pattern length before access"
)]
fn class_match(
    path: &[char], pattern: &[char], path_index: usize, pattern_index: usize,
) -> Option<(bool, usize)> {
    let value = *path.get(path_index)?;
    if value == '/' {
        return None;
    }
    let mut index = pattern_index + 1;
    let negated = pattern
        .get(index)
        .is_some_and(|character| matches!(character, '!' | '^'));
    if negated {
        index += 1;
    }
    let mut matched = false;
    let mut had_item = false;
    while index < pattern.len() && pattern[index] != ']' {
        had_item = true;
        if index + 2 < pattern.len() && pattern[index + 1] == '-' && pattern[index + 2] != ']' {
            matched |= pattern[index] <= value && value <= pattern[index + 2];
            index += 3;
        } else {
            matched |= pattern[index] == value;
            index += 1;
        }
    }
    if index == pattern.len() || !had_item {
        None
    } else {
        Some((matched != negated, index + 1))
    }
}

fn glob_matches(path: &str, pattern: &str, style: Style) -> bool {
    let canonical = |value: &str| {
        match style {
            Style::Posix => value.to_owned(),
            Style::Windows => value.replace('\\', "/"),
        }
    };
    let path: Vec<char> = canonical(path).chars().collect();
    expand_alternatives(&canonical(pattern))
        .iter()
        .any(|pattern| {
            let pattern: Vec<char> = pattern.chars().collect();
            let mut memo = vec![vec![None; pattern.len() + 1]; path.len() + 1];
            glob_match_recursive(&path, &pattern, 0, 0, &mut memo)
        })
}

fn namespaced(path: &str, style: Style) -> String {
    if matches!(style, Style::Posix) || path.is_empty() {
        return path.to_owned();
    }
    let resolved = windows_resolve(&[path.to_owned()]);
    let bytes = resolved.as_bytes();
    let unc = resolved
        .strip_prefix(r"\\")
        .filter(|rest| !matches!(rest.as_bytes().first(), Some(b'?' | b'.')))
        .map(str::to_owned);
    let drive = bytes.first().is_some_and(|byte| drive_letter(*byte))
        && bytes.get(1) == Some(&b':')
        && bytes.get(2) == Some(&b'\\');
    unc.map_or_else(
        || {
            if drive {
                format!(r"\\?\{resolved}")
            } else {
                resolved
            }
        },
        |rest| format!(r"\\?\UNC\{rest}"),
    )
}

fn required_string<'js>(
    ctx: &Ctx<'js>, args: &[Value<'js>], index: usize, name: &str,
) -> Result<String> {
    args.get(index)
        .and_then(Value::as_string)
        .ok_or_else(|| Exception::throw_type(ctx, &format!("{name} must be a string")))?
        .to_string()
}

fn strings<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> Result<Vec<String>> {
    args.iter()
        .enumerate()
        .map(|(index, _)| required_string(ctx, args, index, "path"))
        .collect()
}

fn format_path<'js>(ctx: &Ctx<'js>, value: Value<'js>, style: Style) -> Result<String> {
    if value.is_null() || value.is_array() || value.is_function() {
        return Err(Exception::throw_type(ctx, "pathObject must be an object"));
    }
    let object = value
        .as_object()
        .ok_or_else(|| Exception::throw_type(ctx, "pathObject must be an object"))?;
    let read = |name: &str| -> Result<Option<String>> {
        let value: Value = object.get(name)?;
        if !Coerced::<bool>::from_js(ctx, value.clone())?.0 {
            return Ok(None);
        }
        Ok(Some(Coerced::<String>::from_js(ctx, value)?.0))
    };
    let root = read("root")?;
    let dir = read("dir")?.or_else(|| root.clone());
    let base = if let Some(base) = read("base")? {
        base
    } else {
        let name = read("name")?.unwrap_or_default();
        let ext = read("ext")?.unwrap_or_default();
        format!(
            "{name}{}{ext}",
            if ext.is_empty() || ext.starts_with('.') {
                ""
            } else {
                "."
            }
        )
    };
    let Some(dir) = dir else {
        return Ok(base);
    };
    if root.as_deref() == Some(dir.as_str()) {
        Ok(format!("{dir}{base}"))
    } else {
        Ok(format!(
            "{dir}{}{base}",
            if matches!(style, Style::Posix) {
                '/'
            } else {
                '\\'
            }
        ))
    }
}

fn parts_object<'js>(ctx: &Ctx<'js>, parts: Parts) -> Result<Value<'js>> {
    let object = Object::new(ctx.clone())?;
    object.set("root", parts.root)?;
    object.set("dir", parts.dir)?;
    object.set("base", parts.base)?;
    object.set("ext", parts.ext)?;
    object.set("name", parts.name)?;
    Ok(object.into_value())
}

fn call<'js>(
    ctx: &Ctx<'js>, style: Style, operation: Operation, args: &[Value<'js>],
) -> Result<Value<'js>> {
    match operation {
        Operation::Resolve => resolve(&strings(ctx, args)?, style).into_js(ctx),
        Operation::Normalize => {
            normalize(&required_string(ctx, args, 0, "path")?, style).into_js(ctx)
        }
        Operation::IsAbsolute => {
            is_absolute(&required_string(ctx, args, 0, "path")?, style).into_js(ctx)
        }
        Operation::Join => join(&strings(ctx, args)?, style).into_js(ctx),
        Operation::Relative => {
            relative(
                &required_string(ctx, args, 0, "from")?,
                &required_string(ctx, args, 1, "to")?,
                style,
            )
            .into_js(ctx)
        }
        Operation::ToNamespacedPath => {
            namespaced(&required_string(ctx, args, 0, "path")?, style).into_js(ctx)
        }
        Operation::Dirname => dirname(&required_string(ctx, args, 0, "path")?, style).into_js(ctx),
        Operation::Basename => {
            let path = required_string(ctx, args, 0, "path")?;
            let suffix = match args.get(1) {
                None => None,
                Some(value) if value.is_undefined() => None,
                Some(_) => Some(required_string(ctx, args, 1, "suffix")?),
            };
            basename(&path, suffix.as_deref(), style).into_js(ctx)
        }
        Operation::Extname => extname(&required_string(ctx, args, 0, "path")?, style).into_js(ctx),
        Operation::Format => {
            format_path(
                ctx,
                args.first()
                    .cloned()
                    .unwrap_or_else(|| Value::new_undefined(ctx.clone())),
                style,
            )?
            .into_js(ctx)
        }
        Operation::Parse => {
            parts_object(ctx, parse(&required_string(ctx, args, 0, "path")?, style))
        }
        Operation::MatchesGlob => {
            glob_matches(
                &required_string(ctx, args, 0, "path")?,
                &required_string(ctx, args, 1, "pattern")?,
                style,
            )
            .into_js(ctx)
        }
    }
}

fn function<'js>(
    ctx: &Ctx<'js>, style: Style, operation: Operation, name: &str,
) -> Result<Function<'js>> {
    Function::new(
        ctx.clone(),
        move |ctx: Ctx<'js>, Rest(args): Rest<Value<'js>>| call(&ctx, style, operation, &args),
    )?
    .with_name(name)?
    .with_length(operation.length())
}

fn namespace<'js>(ctx: &Ctx<'js>, style: Style) -> Result<Object<'js>> {
    let object = Object::new(ctx.clone())?;
    for (name, operation) in [
        ("resolve", Operation::Resolve),
        ("normalize", Operation::Normalize),
        ("isAbsolute", Operation::IsAbsolute),
        ("join", Operation::Join),
        ("relative", Operation::Relative),
        ("toNamespacedPath", Operation::ToNamespacedPath),
        ("dirname", Operation::Dirname),
        ("basename", Operation::Basename),
        ("extname", Operation::Extname),
        ("format", Operation::Format),
        ("parse", Operation::Parse),
        ("matchesGlob", Operation::MatchesGlob),
    ] {
        object.set(name, function(ctx, style, operation, name)?)?;
    }
    let (separator, delimiter) = match style {
        Style::Posix => ("/", ":"),
        Style::Windows => ("\\", ";"),
    };
    object.set("sep", separator)?;
    object.set("delimiter", delimiter)?;
    Ok(object)
}

fn namespaces<'js>(ctx: &Ctx<'js>) -> Result<(Object<'js>, Object<'js>)> {
    let posix = namespace(ctx, Style::Posix)?;
    let windows = namespace(ctx, Style::Windows)?;
    posix.set("posix", posix.clone())?;
    posix.set("windows", windows.clone())?;
    windows.set("posix", posix.clone())?;
    windows.set("windows", windows.clone())?;
    Ok((posix, windows))
}

#[rquickjs::module]
pub mod path {
    use rquickjs::{
        Ctx, Result, Value,
        module::{Declarations, Exports},
    };

    #[qjs(declare)]
    pub fn declare(declarations: &Declarations) -> Result<()> {
        for name in super::EXPORTS {
            declarations.declare(name)?;
        }
        Ok(())
    }

    #[qjs(evaluate)]
    pub fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> Result<()> {
        let (posix, windows) = super::namespaces(ctx)?;
        let selected = if cfg!(windows) { windows } else { posix };
        for name in super::EXPORTS {
            if name == "default" {
                exports.export(name, selected.clone())?;
            } else {
                let value: Value = selected.get(name)?;
                exports.export(name, value)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Parts, Style, basename, dirname, extname, glob_matches, join, parse, posix_normalize,
        relative, windows_normalize,
    };

    #[test]
    fn normalizes_both_syntaxes() {
        assert_eq!(
            posix_normalize("./fixtures///b/../b/c.js"),
            "fixtures/b/c.js"
        );
        assert_eq!(
            posix_normalize("../../../foo/../../../bar"),
            "../../../../../bar"
        );
        assert_eq!(windows_normalize("a//b//../b"), r"a\b");
        assert_eq!(
            windows_normalize("//server/share/dir/../../../file"),
            r"\\server\share\file"
        );
    }

    #[test]
    fn composes_and_splits_paths() {
        assert_eq!(
            join(&["/srv".to_owned(), "app/../data".to_owned()], Style::Posix),
            "/srv/data"
        );
        assert_eq!(basename("/foo/bar.txt", Some(".txt"), Style::Posix), "bar");
        assert_eq!(extname("/foo/.config.json", Style::Posix), ".json");
        assert_eq!(dirname(r"C:\foo\bar", Style::Windows), r"C:\foo");
        assert_eq!(parse("/home/user/file.txt", Style::Posix), Parts {
            root: "/".to_owned(),
            dir:  "/home/user".to_owned(),
            base: "file.txt".to_owned(),
            ext:  ".txt".to_owned(),
            name: "file".to_owned(),
        });
    }

    #[test]
    fn derives_relative_paths_and_matches_globs() {
        assert_eq!(
            relative("/data/project/src", "/data/project/tests", Style::Posix),
            "../tests"
        );
        assert!(glob_matches(
            "src/lib/path.rs",
            "src/**/{path,url}.rs",
            Style::Posix
        ));
        assert!(glob_matches("foo.js", "@(foo|bar).js", Style::Posix));
        assert!(!glob_matches("src/.env", "src/*", Style::Posix));
    }
}
