pub fn tag<'s>(
    res: &'s str,
    start_tag: impl AsRef<str>,
    end_tag: impl AsRef<str>,
) -> Option<(String, &'s str)> {
    let Some((left, right)) = res.split_once(end_tag.as_ref()) else {
        return None;
    };
    let Some((lleft, lright)) = left.split_once(start_tag.as_ref()) else {
        return Some((right.trim_start().into(), left.trim()));
    };
    return Some((
        format!("{}\n{}", lleft.trim_end(), right.trim_start()).trim().into(),
        lright.trim(),
    ));
}

#[cfg(test)]
mod test {
    use crate::llm::dialog::parse::tag;

    #[test]
    fn tag_middle() {
        let res = "foo\n<|think|>\nbar\n<|/think|>\nbaz";
        let (rest, reasoning) = tag(res, "<|think|>", "<|/think|>").unwrap();
        assert_eq!(rest, "foo\nbaz");
        assert_eq!(reasoning, "bar");
    }

    #[test]
    fn tag_end() {
        let res = "foo\n<|think|>\nbar\n<|/think|>";
        let (rest, reasoning) = tag(res, "<|think|>", "<|/think|>").unwrap();
        assert_eq!(rest, "foo");
        assert_eq!(reasoning, "bar");
    }

    #[test]
    fn tag_start() {
        let res = "<|think|>\nfoo\n<|/think|>\nbar";
        let (rest, reasoning) = tag(res, "<|think|>", "<|/think|>").unwrap();
        assert_eq!(rest, "bar");
        assert_eq!(reasoning, "foo");
    }
}
