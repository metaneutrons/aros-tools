use super::{join_continuations, join_continuations_with_source_ranges};

fn assert_partition(content: &str) {
    let (joined, ranges) = join_continuations_with_source_ranges(content);

    assert_eq!(joined, join_continuations(content));
    assert_eq!(ranges.len(), joined.lines().count());
    for range in &ranges {
        assert!(content.is_char_boundary(range.start));
        assert!(content.is_char_boundary(range.end));
    }

    if content.is_empty() {
        assert!(ranges.is_empty());
    } else {
        assert_eq!(ranges.first().unwrap().start, 0);
        assert_eq!(ranges.last().unwrap().end, content.len());
        for pair in ranges.windows(2) {
            assert_eq!(pair[0].end, pair[1].start);
        }
    }
}

#[test]
fn unicode_continuations_map_to_complete_physical_source_spans() {
    let content = "α\\ \n  β\r\n\nγ\\\t\n \tδ\nlast";
    let (joined, ranges) = join_continuations_with_source_ranges(content);

    assert_eq!(joined, join_continuations(content));
    assert_eq!(
        joined.lines().collect::<Vec<_>>(),
        ["α β", "", "γ δ", "last"]
    );
    assert_eq!(ranges.len(), joined.lines().count());
    let source_parts = ranges
        .iter()
        .map(|range| &content[range.clone()])
        .collect::<Vec<_>>();
    assert_eq!(
        source_parts,
        ["α\\ \n  β\r\n", "\n", "γ\\\t\n \tδ\n", "last"]
    );
    assert_partition(content);
}

#[test]
fn empty_and_noncontinued_shapes_keep_original_join_semantics() {
    for content in [
        "",
        "\n",
        "first",
        "first\n",
        "first\n\nthird",
        "left\\ right\nmiddle\r\nlast",
    ] {
        assert_partition(content);
    }

    let (joined, ranges) = join_continuations_with_source_ranges("first\n\nthird\n");
    assert_eq!(joined.lines().collect::<Vec<_>>(), ["first", "", "third"]);
    assert_eq!(
        ranges
            .iter()
            .map(|range| &"first\n\nthird\n"[range.clone()])
            .collect::<Vec<_>>(),
        ["first\n", "\n", "third\n"]
    );
}
