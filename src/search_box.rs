//! Inline completion for the search box: `@operators` and `#tags`, plus the
//! box's own editing state (value, cursor, ghost suggestion).

/// bearcli's search operators, most used first. The parenthesised ones stop
/// at `(` so the argument is left to type; X-day forms offer 7 as the digit.
pub const OPERATORS: [&str; 25] = [
    "@todo",
    "@done",
    "@task",
    "@today",
    "@yesterday",
    "@last7days",
    "@date(",
    "@ctoday",
    "@created7days",
    "@cdate(",
    "@title",
    "@tagged",
    "@untagged",
    "@pinned",
    "@images",
    "@files",
    "@attachments",
    "@code",
    "@locked",
    "@readonly",
    "@empty",
    "@untitled",
    "@wikilinks",
    "@backlinks",
    "@ocr",
];

/// The cheat-sheet row shown under the box while it is open.
pub const HINT: &str = "\"phrase\"  -term  #tag  #*/subtag  @todo  @title  @today  @last7days  @pinned  ·  tab or → accepts a completion";

/// `tags` plus every ancestor path, order kept, no duplicates.
pub fn with_parents(tags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tag in tags {
        let parts: Vec<&str> = tag.split('/').collect();
        for i in 1..=parts.len() {
            let path = parts[..i].join("/");
            if !out.contains(&path) {
                out.push(path);
            }
        }
    }
    out
}

/// How the tag is typed in a query: multi-word tags close with `#`.
fn tag_token(tag: &str) -> String {
    if tag.contains(' ') {
        format!("#{tag}#")
    } else {
        format!("#{tag}")
    }
}

fn first_prefixed<'a>(
    candidates: impl IntoIterator<Item = &'a str>,
    prefix: &str,
) -> Option<String> {
    let folded = prefix.to_lowercase();
    candidates
        .into_iter()
        .find(|c| {
            let lower = c.to_lowercase();
            lower.starts_with(&folded) && lower != folded
        })
        .map(|c| c.to_string())
}

/// Every tag's tail below its first segment: `a/b/c` gives `b/c` and `c`.
fn tails(tags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tag in tags {
        let parts: Vec<&str> = tag.split('/').collect();
        for i in 1..parts.len() {
            let tail = parts[i..].join("/");
            if !out.contains(&tail) {
                out.push(tail);
            }
        }
    }
    out
}

/// The completed form of `token`, or None when nothing applies.
pub fn complete_token(token: &str, tags: &[String]) -> Option<String> {
    if token.starts_with('@') {
        return first_prefixed(OPERATORS.iter().copied(), token);
    }
    let bang = token.starts_with("!#");
    if !bang && !token.starts_with('#') {
        return None;
    }
    let body = if bang { &token[2..] } else { &token[1..] };
    let prefix = if bang { "!" } else { "" };
    let tag_tails = tails(tags);
    if let Some(rest) = body.strip_prefix("*/") {
        return first_prefixed(tag_tails.iter().map(String::as_str), rest)
            .map(|found| format!("{prefix}#*/{found}"));
    }
    if let Some(found) = first_prefixed(tags.iter().map(String::as_str), body) {
        return Some(format!("{prefix}{}", tag_token(&found)));
    }
    // No path starts this way: offer the sub-tag form, which is how Bear
    // reaches a tag by its tail (`#Build` matches nothing, `#*/Build` does).
    first_prefixed(tag_tails.iter().map(String::as_str), body)
        .map(|found| format!("{prefix}#*/{found}"))
}

/// `value` with its last token completed, or None.
pub fn complete(value: &str, tags: &[String]) -> Option<String> {
    if value.is_empty() || value.ends_with(' ') {
        return None;
    }
    let (head, token) = match value.rfind(' ') {
        Some(i) => (&value[..=i], &value[i + 1..]),
        None => ("", value),
    };
    complete_token(token, tags).map(|done| format!("{head}{done}"))
}

/// The search box: what is typed, where the cursor is, and the ghost
/// completion after it. Open with the query it last ran, dimmed until focused.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchBox {
    pub open: bool,
    pub value: String,
    pub cursor: usize,
    pub suggestion: Option<String>,
}

impl SearchBox {
    pub fn open_with(&mut self, prefill: &str) {
        self.open = true;
        self.value = prefill.to_string();
        self.cursor = self.value.chars().count();
        self.suggestion = None;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.value.clear();
        self.cursor = 0;
        self.suggestion = None;
    }

    fn byte_index(&self, chars: usize) -> usize {
        self.value
            .char_indices()
            .nth(chars)
            .map(|(i, _)| i)
            .unwrap_or(self.value.len())
    }

    pub fn insert(&mut self, ch: char) {
        let at = self.byte_index(self.cursor);
        self.value.insert(at, ch);
        self.cursor += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let at = self.byte_index(self.cursor - 1);
        self.value.remove(at);
        self.cursor -= 1;
    }

    pub fn delete(&mut self) {
        if self.cursor < self.value.chars().count() {
            let at = self.byte_index(self.cursor);
            self.value.remove(at);
        }
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    /// `→`: accept the ghost completion when the cursor sits at the end,
    /// otherwise move right. Returns whether a completion was accepted.
    pub fn right(&mut self) -> bool {
        if self.cursor >= self.value.chars().count() {
            return self.accept();
        }
        self.cursor += 1;
        false
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.value.chars().count();
    }

    /// Take the ghost completion as the value. False when there is none.
    pub fn accept(&mut self) -> bool {
        match self.suggestion.take() {
            Some(done) if done.chars().count() > self.value.chars().count() => {
                self.value = done;
                self.cursor = self.value.chars().count();
                true
            }
            _ => false,
        }
    }

    /// Recompute the ghost text from the current value and `tags` (priority order).
    pub fn refresh_suggestion(&mut self, tags: &[String]) {
        self.suggestion = complete(&self.value, &with_parents(tags));
    }

    /// The characters of the suggestion beyond what is typed.
    pub fn ghost(&self) -> &str {
        match &self.suggestion {
            Some(done) if done.starts_with(&self.value) => &done[self.value.len()..],
            _ => "",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|i| i.to_string()).collect()
    }

    fn tags() -> Vec<String> {
        s(&[
            "work/planning",
            "home/garden",
            "home",
            "techne/serial numbers",
            "diary",
        ])
    }

    #[test]
    fn operators_complete_in_list_order() {
        assert_eq!(complete("@to", &tags()).unwrap(), "@todo");
        assert_eq!(complete("@d", &tags()).unwrap(), "@done");
        assert_eq!(complete("@t", &tags()).unwrap(), "@todo");
        assert_eq!(complete("@date", &tags()).unwrap(), "@date(");
        assert_eq!(complete("@cd", &tags()).unwrap(), "@cdate(");
    }

    #[test]
    fn tags_complete_including_paths_and_forms() {
        let all = with_parents(&tags());
        assert_eq!(complete("#wo", &all).unwrap(), "#work");
        assert_eq!(complete("#work/p", &all).unwrap(), "#work/planning");
        assert_eq!(complete("#home/g", &all).unwrap(), "#home/garden");
        assert_eq!(complete("!#ho", &all).unwrap(), "!#home");
        assert_eq!(complete("#*/g", &all).unwrap(), "#*/garden");
        assert_eq!(complete("#*/plan", &all).unwrap(), "#*/planning");
        assert_eq!(
            complete("#techne/ser", &all).unwrap(),
            "#techne/serial numbers#"
        );
        assert_eq!(complete("bulbs @to", &tags()).unwrap(), "bulbs @todo");
        assert_eq!(complete("Bulbs #WO", &all).unwrap(), "Bulbs #work");
    }

    #[test]
    fn nothing_for_plain_words_unknown_prefixes_or_trailing_space() {
        assert!(complete("bulbs", &tags()).is_none());
        assert!(complete("@zzz", &tags()).is_none());
        assert!(complete("#nope", &tags()).is_none());
        assert!(complete("@todo ", &tags()).is_none());
        assert!(complete("", &tags()).is_none());
        assert!(complete("@todo", &tags()).is_none());
    }

    #[test]
    fn case_insensitive_match_keeps_the_candidate_spelling_and_priority() {
        let all = with_parents(&tags());
        assert_eq!(complete("@TO", &tags()).unwrap(), "@todo");
        assert_eq!(complete("#Ho", &all).unwrap(), "#home");
        assert_eq!(complete("#Home", &all).unwrap(), "#home/garden");
        assert_eq!(complete("#h", &s(&["home", "health"])).unwrap(), "#home");
        assert_eq!(complete("#h", &s(&["health", "home"])).unwrap(), "#health");
        assert_eq!(
            with_parents(&s(&["a/b/c", "a/b", "x"])),
            s(&["a", "a/b", "a/b/c", "x"])
        );
        for op in [
            "@todo",
            "@done",
            "@task",
            "@title",
            "@pinned",
            "@untagged",
            "@date(",
            "@ocr",
            "@backlinks",
        ] {
            assert!(OPERATORS.contains(&op));
        }
        let all = with_parents(&s(&["kybernetes/Build", "kybernetes/Seasons/Decode"]));
        assert_eq!(complete("#Bui", &all).unwrap(), "#*/Build");
        assert_eq!(complete("#seas", &all).unwrap(), "#*/Seasons");
        assert_eq!(complete("#kyb", &all).unwrap(), "#kybernetes");
    }

    #[test]
    fn box_edits_and_accepts() {
        let mut b = SearchBox::default();
        b.open_with("");
        for ch in "bulbs #ho".chars() {
            b.insert(ch);
        }
        b.refresh_suggestion(&tags());
        assert_eq!(b.suggestion.as_deref(), Some("bulbs #home"));
        assert_eq!(b.ghost(), "me");
        assert!(b.right());
        assert_eq!(b.value, "bulbs #home");
        b.refresh_suggestion(&tags());
        assert_eq!(b.suggestion.as_deref(), Some("bulbs #home/garden"));
        b.backspace();
        assert_eq!(b.value, "bulbs #hom");
        b.home();
        assert!(!b.right(), "right in the middle only moves");
        assert_eq!(b.cursor, 1);
        b.close();
        assert!(!b.open && b.value.is_empty());
    }
}
