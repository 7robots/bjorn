//! What in a Bear search query is matched against note text.
//!
//! bearcli does the searching; this module only recovers the terms a query
//! would have matched so the reader and the list can highlight them.
//! Operators (`@todo`, `@date(2026-01-01)`), tags (`#tag`, `!#tag`, `#*/sub`,
//! `#multi word#`) and negations (`-term`) narrow the result set but match no
//! text, so they are dropped.

use std::sync::LazyLock;

use fancy_regex::Regex as FancyRegex;
use regex::Regex;

use crate::render::TAG_TOKEN_PATTERN;

static TOKEN_RE: LazyLock<FancyRegex> = LazyLock::new(|| {
    let pattern = format!(
        r#"(?P<neg>-)?(?:"(?P<phrase>[^"]*)"|@(?P<op>[\w-]+)(?:\([^)]*\))?|!?(?P<tag>{TAG_TOKEN_PATTERN})|(?P<word>\S+))"#
    );
    FancyRegex::new(&pattern).unwrap()
});

/// Bare words and quoted phrases from `query`, in order, without duplicates.
/// Empty when the query only narrows (operators, tags).
pub fn terms(query: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for caps in TOKEN_RE.captures_iter(query).flatten() {
        if caps.name("neg").is_some() || caps.name("op").is_some() || caps.name("tag").is_some() {
            continue;
        }
        let term = caps
            .name("phrase")
            .or_else(|| caps.name("word"))
            .map(|m| m.as_str().trim())
            .unwrap_or("");
        if !term.is_empty()
            && !found
                .iter()
                .any(|f| f.eq_ignore_ascii_case(term) || f.to_lowercase() == term.to_lowercase())
        {
            found.push(term.to_string());
        }
    }
    found
}

/// One case-insensitive regex matching any of `words` as substrings, longest
/// first so a phrase wins over a word it contains. None when empty.
pub fn pattern(words: &[String]) -> Option<Regex> {
    if words.is_empty() {
        return None;
    }
    let mut ordered: Vec<&String> = words.iter().collect();
    ordered.sort_by_key(|w| std::cmp::Reverse(w.chars().count()));
    let alternation = ordered
        .iter()
        .map(|w| regex::escape(w))
        .collect::<Vec<_>>()
        .join("|");
    Regex::new(&format!("(?i){alternation}")).ok()
}

pub fn query_pattern(query: &str) -> Option<Regex> {
    pattern(&terms(query))
}

static BARE_TAG_RE: LazyLock<FancyRegex> = LazyLock::new(|| {
    FancyRegex::new(r"(?<![\w!#*/])#(?!\*/)([^\s#]+(?:[^#\n]*?#(?=\s|$))?)").unwrap()
});

/// Bear matches `#name` against full tag paths only; `#*/name` reaches a
/// sub-tag by its tail. A bare `#name` that is no tag path but is the tail of
/// one becomes `#*/name`. `!#`, `#*/` and known paths are left alone.
pub fn rewrite_subtags(query: &str, tags: &[String]) -> String {
    let paths: std::collections::HashSet<String> = tags.iter().map(|t| t.to_lowercase()).collect();
    let mut tails: std::collections::HashSet<String> = std::collections::HashSet::new();
    for t in tags {
        let parts: Vec<&str> = t.split('/').collect();
        for i in 1..parts.len() {
            tails.insert(parts[i..].join("/").to_lowercase());
        }
    }
    let mut out = String::with_capacity(query.len() + 8);
    let mut last = 0;
    for caps in BARE_TAG_RE.captures_iter(query).flatten() {
        let whole = caps.get(0).unwrap();
        let body = &caps[1];
        let key = body.trim_end_matches('#').to_lowercase();
        out.push_str(&query[last..whole.start()]);
        if paths.contains(&key) || !tails.contains(&key) {
            out.push_str(whole.as_str());
        } else {
            out.push_str(&format!("#*/{body}"));
        }
        last = whole.end();
    }
    out.push_str(&query[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|i| i.to_string()).collect()
    }

    #[test]
    fn words_phrases_operators_tags_negations() {
        assert_eq!(
            terms(r#"bulbs "spring planting" garden"#),
            s(&["bulbs", "spring planting", "garden"])
        );
        assert_eq!(
            terms("@todo @title @date(2026-01-01) @last7days @ctoday foo"),
            s(&["foo"])
        );
        assert_eq!(
            terms("#work !#work #*/planning #multi word# foo"),
            s(&["foo"])
        );
        assert_eq!(terms(r#"foo -bar -"not this" baz"#), s(&["foo", "baz"]));
        assert_eq!(terms("@title foo"), s(&["foo"]));
        assert!(terms("@todo #work -draft").is_empty());
        assert!(pattern(&[]).is_none());
        assert!(query_pattern("@todo").is_none());
        assert_eq!(terms("Foo foo FOO"), s(&["Foo"]));
    }

    #[test]
    fn pattern_escapes_ignores_case_and_prefers_longest() {
        let p = pattern(&s(&["a.b", "c+d"])).unwrap();
        assert!(p.is_match("xx A.B yy"));
        assert!(!p.is_match("xx AxB yy"));
        assert!(p.is_match("C+D"));
        let p = pattern(&s(&["plan", "spring planting"])).unwrap();
        assert_eq!(
            p.find("a spring planting day").unwrap().as_str(),
            "spring planting"
        );
        assert!(query_pattern("bulb").unwrap().is_match("lightbulbs"));
    }

    #[test]
    fn bare_subtags_are_rewritten() {
        let tags = s(&[
            "kybernetes",
            "kybernetes/Build",
            "kybernetes/Seasons/Decode",
            "home",
            "home/garden",
        ]);
        assert_eq!(rewrite_subtags("#Build", &tags), "#*/Build");
        assert_eq!(
            rewrite_subtags("robot #build notes", &tags),
            "robot #*/build notes"
        );
        assert_eq!(
            rewrite_subtags("#Seasons/Decode", &tags),
            "#*/Seasons/Decode"
        );
        assert_eq!(
            rewrite_subtags("#kybernetes/Build", &tags),
            "#kybernetes/Build"
        );
        assert_eq!(rewrite_subtags("#home", &tags), "#home");
        assert_eq!(rewrite_subtags("!#Build", &tags), "!#Build");
        assert_eq!(rewrite_subtags("#*/Build", &tags), "#*/Build");
        assert_eq!(rewrite_subtags("#nothing", &tags), "#nothing");
        assert_eq!(rewrite_subtags("plain words", &tags), "plain words");
    }
}
