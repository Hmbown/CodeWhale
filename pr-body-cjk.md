## Bug

When `assistant_text` contains CJK characters (3-byte UTF-8), the byte slice
`&assistant_text[..SUMMARY_LIMIT.saturating_sub(3)]` (byte 277) can land in
the middle of a multi-byte sequence, causing a panic:

```
byte index 277 is not a char boundary (it is inside a 3-byte UTF-8 sequence)
```

## Fix

Replace the raw byte-index slice with `truncate_with_ellipsis()` which already
finds the nearest safe char boundary via `char_indices()`.

## Change

```diff
-format!("{}...", &assistant_text[..SUMMARY_LIMIT.saturating_sub(3)])
+crate::utils::truncate_with_ellipsis(&assistant_text, SUMMARY_LIMIT, "...")
```

1 line changed.

## Files

- crates/tui/src/runtime_threads.rs:1437
