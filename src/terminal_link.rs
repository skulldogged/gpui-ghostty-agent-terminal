use fancy_regex::Regex;
use std::{ops::Range, sync::OnceLock};

const LINK_PATTERN: &str = concat!(
    r"(?:https?://|mailto:|ftp://|file:|ssh:|git://|ssh://|tel:|magnet:|ipfs://|ipns://|gemini://|gopher://|news:)",
    r"(?:\[[0-9a-fA-F:]+\](?::[0-9]+)?|[\w\-.~:/?#@!$&*+,;=%]+(?:[\(\[]\w*[\)\]])?)+",
    r"(?<![:,.])",
    r"|",
    r"(?:\.\./|\./|(?<!\w)~/|(?:[\w][\w\-.]*/)*(?<!\w)\$[A-Za-z_]\w*/|\.[\w][\w\-.]*/|(?<![\w~/])/(?!/))",
    r"(?:",
    r"(?=[\w\-.~:/?#@!$&*+;=%]*\.)[\w\-.~:/?#@!$&*+;=%]+(?:(?<!:) (?!\w+://)(?!\.{0,2}/)(?!~/)[\w\-.~:/?#@!$&*+;=%]*[/\.])*(?<!:)",
    r"|",
    r"(?![\w\-.~:/?#@!$&*+;=%]*\.)[\w\-.~:/?#@!$&*+;=%]+(?:(?<!:) (?!\w+://)(?!\.{0,2}/)(?!~/)[\w\-.~:/?#@!$&*+;=%]+)*(?<!:)",
    r")",
    r"|",
    r"(?=[\w\-.~:/?#@!$&*+;=%]*\.)(?<!\$\d*)(?<!\w)[\w][\w\-.]*/[\w\-.~:/?#@!$&*+;=%]+(?<!:)"
);

fn matcher() -> Result<&'static Regex, String> {
    static MATCHER: OnceLock<Result<Regex, String>> = OnceLock::new();
    MATCHER
        .get_or_init(|| Regex::new(LINK_PATTERN).map_err(|error| error.to_string()))
        .as_ref()
        .map_err(Clone::clone)
}

pub(crate) fn match_at(text: &str, byte_offset: usize) -> Result<Option<Range<usize>>, String> {
    for result in matcher()?.find_iter(text) {
        let found = result.map_err(|error| error.to_string())?;
        if found.start() <= byte_offset && byte_offset < found.end() {
            return Ok(Some(found.start()..found.end()));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    #[test]
    fn scheme_links_exclude_sentence_delimiters() {
        for (text, expected) in [
            ("https://google.com:", "https://google.com"),
            ("https://google.com.", "https://google.com"),
            ("https://google.com,", "https://google.com"),
            ("(https://google.com)", "https://google.com"),
        ] {
            let range = super::match_at(text, 10)
                .expect("match link")
                .expect("link under pointer");
            assert_eq!(&text[range], expected);
        }
    }

    #[test]
    fn link_match_must_contain_the_pointer() {
        let text = "https://google.com: trailing";
        assert!(super::match_at(text, 18).expect("match URL").is_none());
    }

    #[test]
    fn matches_ghostty_urls_and_paths() {
        for (text, offset, expected) in [
            (
                "query https://example.com?one=1&two=2 end",
                20,
                "https://example.com?one=1&two=2",
            ),
            ("open ../src/main.rs now", 10, "../src/main.rs"),
            ("IPv6 http://[::1]:3000 ready", 16, "http://[::1]:3000"),
        ] {
            let range = super::match_at(text, offset)
                .expect("match link")
                .expect("link under pointer");
            assert_eq!(&text[range], expected);
        }
    }
}
