/// Convert text to a URL-safe slug (lowercase, hyphens only).
pub fn slugify(text: &str) -> String {
    let lower = text.to_lowercase();
    let mut slug = String::new();
    let mut prev_hyphen = false;

    for c in lower.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
            prev_hyphen = false;
        } else if !prev_hyphen {
            slug.push('-');
            prev_hyphen = true;
        }
    }

    slug.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slugify_basic() {
        assert_eq!(slugify("Hello World"), "hello-world");
    }

    #[test]
    fn test_slugify_special_chars() {
        assert_eq!(slugify("Fix Bug #1!"), "fix-bug-1");
    }

    #[test]
    fn test_slugify_multiple_separators() {
        assert_eq!(slugify("hello   world"), "hello-world");
    }

    #[test]
    fn test_slugify_leading_trailing() {
        assert_eq!(slugify("  hello  "), "hello");
    }

    #[test]
    fn test_slugify_already_slug() {
        assert_eq!(slugify("fix-bug"), "fix-bug");
    }
}
