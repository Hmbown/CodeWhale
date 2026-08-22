use super::*;
use crate::models::Role;

/// A 1x1 PNG, as bytes rather than a fixture file so the encoding tests
/// have no filesystem dependency.
pub(crate) const PNG_1X1: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
    0x42, 0x60, 0x82,
];

#[test]
fn sniffs_every_accepted_format_from_magic_bytes() {
    assert_eq!(sniff_media_type(PNG_1X1), Some("image/png"));
    assert_eq!(
        sniff_media_type(&[0xff, 0xd8, 0xff, 0xe0, 0x00]),
        Some("image/jpeg")
    );
    assert_eq!(sniff_media_type(b"GIF89a....."), Some("image/gif"));
    assert_eq!(sniff_media_type(b"GIF87a....."), Some("image/gif"));
    assert_eq!(
        sniff_media_type(b"RIFF\x00\x00\x00\x00WEBPVP8 "),
        Some("image/webp")
    );
}

#[test]
fn sniffing_ignores_the_extension_and_believes_the_bytes() {
    // A JPEG named .png must be declared image/jpeg, or the provider
    // rejects the media-type mismatch.
    let jpeg = [0xff, 0xd8, 0xff, 0xe0, 0x11, 0x22];
    let attached = encode_image_bytes(&jpeg, "screenshot.png").expect("attach");
    assert_eq!(attached.media_type, "image/jpeg");
    assert!(attached.data_url.starts_with("data:image/jpeg;base64,"));
}

#[test]
fn lowercase_read_image_preparation_is_typed_and_bounded() {
    let prepared = prepare_tool_image_bytes(PNG_1X1, "image/png");
    assert_eq!(prepared.note, "Read image file [image/png]");
    let codewhale_tools::ToolResultContentBlock::Image { mime_type, data } =
        prepared.block.expect("typed image");
    assert_eq!(mime_type, "image/png");
    assert_eq!(STANDARD.decode(data).expect("base64"), PNG_1X1);

    let omitted = prepare_tool_image_bytes(b"BMnot-a-safe-bitmap", "image/bmp");
    assert!(omitted.block.is_none());
    assert!(omitted.note.contains("Image omitted"), "{}", omitted.note);
}

#[test]
fn blind_route_removes_nested_tool_result_image() {
    let mut messages = vec![crate::models::Message {
        role: Role::User,
        content: vec![ContentBlock::ToolResult {
            tool_use_id: "call-image".to_string(),
            content: "Read image file [image/png]".to_string(),
            is_error: None,
            content_blocks: Some(vec![serde_json::json!({
                "type": "image",
                "mime_type": "image/png",
                "data": "QUJD",
            })]),
        }],
    }];

    assert_eq!(
        strip_images_when_unsupported(&mut messages, SupportState::Unsupported, "text-only",),
        1
    );
    let ContentBlock::ToolResult {
        content,
        content_blocks,
        ..
    } = &messages[0].content[0]
    else {
        panic!("tool result")
    };
    assert!(content_blocks.is_none());
    assert!(content.contains("text-only"), "{content}");
}

#[test]
fn copy_projection_never_contains_inline_image_bytes() {
    const SENTINEL: &str = "U0VOU0lUSVZFX0JBU0U2NA==";
    let projected = safe_tool_result_content_blocks(Some(&[serde_json::json!({
        "type": "image",
        "mime_type": "image/png",
        "data": SENTINEL,
    })]))
    .expect("projection");
    let encoded = serde_json::to_string(&projected).expect("json");
    assert!(!encoded.contains(SENTINEL), "{encoded}");
    assert!(
        encoded.contains("inline_or_local_image_payload"),
        "{encoded}"
    );
}

#[test]
fn riff_that_is_not_webp_is_not_an_image() {
    // A WAV file is also RIFF. Matching on "RIFF" alone would attach audio.
    assert_eq!(sniff_media_type(b"RIFF\x00\x00\x00\x00WAVEfmt "), None);
}

#[test]
fn encodes_a_png_to_a_data_url_that_round_trips() {
    let attached = encode_image_bytes(PNG_1X1, "shot.png").expect("attach");
    assert_eq!(attached.media_type, "image/png");
    assert_eq!(attached.source_bytes, PNG_1X1.len());

    let (media_type, payload) = parse_data_url(&attached.data_url).expect("parse");
    assert_eq!(media_type, "image/png");
    assert_eq!(STANDARD.decode(payload).expect("decode"), PNG_1X1);
}

#[test]
fn rejects_a_file_over_the_size_limit() {
    let oversized = vec![0u8; MAX_IMAGE_BYTES + 1];
    let error = encode_image_bytes(&oversized, "huge.png").expect_err("must reject");
    assert!(
        matches!(error, ImageAttachError::TooLarge { .. }),
        "got {error:?}"
    );
    let rendered = error.to_string();
    assert!(rendered.contains("5.0 MB"), "{rendered}");
    assert!(rendered.contains("huge.png"), "{rendered}");
}

#[test]
fn accepts_a_file_exactly_at_the_size_limit() {
    // The boundary is inclusive; an off-by-one here would reject images
    // the providers accept.
    let mut at_limit = PNG_1X1.to_vec();
    at_limit.resize(MAX_IMAGE_BYTES, 0);
    assert!(encode_image_bytes(&at_limit, "edge.png").is_ok());
}

#[test]
fn rejects_a_real_image_in_an_unsupported_format_by_name() {
    for (bytes, name) in [
        (b"BM\x00\x00\x00\x00".as_slice(), "BMP"),
        (b"II\x2a\x00extra".as_slice(), "TIFF"),
        (b"MM\x00\x2aextra".as_slice(), "TIFF"),
        (b"<svg xmlns=".as_slice(), "SVG"),
        (b"%PDF-1.7".as_slice(), "PDF"),
    ] {
        let error = encode_image_bytes(bytes, "f").expect_err("must reject");
        match error {
            ImageAttachError::UnsupportedFormat { detected, .. } => {
                assert_eq!(detected, name);
            }
            other => panic!("expected UnsupportedFormat for {name}, got {other:?}"),
        }
    }
}

#[test]
fn rejects_a_file_that_is_not_an_image_at_all() {
    let error = encode_image_bytes(b"#!/bin/sh\necho hi\n", "script.png").expect_err("must reject");
    assert!(
        matches!(error, ImageAttachError::NotAnImage { .. }),
        "got {error:?}"
    );
}

#[test]
fn rejects_an_empty_file() {
    let error = encode_image_bytes(b"", "empty.png").expect_err("must reject");
    assert!(
        matches!(error, ImageAttachError::Empty { .. }),
        "got {error:?}"
    );
}

#[test]
fn parses_and_rejects_data_urls() {
    assert_eq!(
        parse_data_url("data:image/png;base64,QUJD"),
        Some(("image/png", "QUJD"))
    );
    // Not base64-tagged: Anthropic has no shape for a raw data URL.
    assert_eq!(parse_data_url("data:image/png,QUJD"), None);
    // Remote URLs are a different source type, not a malformed data URL.
    assert_eq!(parse_data_url("https://example.com/a.png"), None);
    // Degenerate forms must not produce an empty base64 payload that the
    // provider would reject with an opaque error.
    assert_eq!(parse_data_url("data:;base64,QUJD"), None);
    assert_eq!(parse_data_url("data:image/png;base64,"), None);
    assert_eq!(parse_data_url("data:image/png;base64"), None);
}

#[test]
fn classifies_remote_urls() {
    assert!(is_remote_image_url("https://example.com/a.png"));
    assert!(is_remote_image_url("http://example.com/a.png"));
    assert!(!is_remote_image_url("data:image/png;base64,QUJD"));
    assert!(!is_remote_image_url("file:///tmp/a.png"));
}

fn message_with_image(url: &str) -> crate::models::Message {
    crate::models::Message {
        role: Role::User,
        content: vec![
            ContentBlock::ImageUrl {
                image_url: ImageUrlContent {
                    url: url.to_string(),
                },
            },
            ContentBlock::Text {
                text: "what is this?".to_string(),
                cache_control: None,
            },
        ],
    }
}

fn write_png(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, PNG_1X1).expect("write fixture");
    path
}

#[test]
fn expands_a_placeholder_into_an_image_block() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_png(dir.path(), "shot.png");
    let text = format!("look at this\n[Attached image: {}]", path.display());

    let expanded = expand_attachment_blocks(&text);

    assert!(expanded.notices.is_empty(), "{expanded:?}");
    // Bracketed: open tag naming the path, the image, close tag.
    assert_eq!(expanded.blocks.len(), 3, "{expanded:?}");
    match &expanded.blocks[0] {
        ContentBlock::Text { text, .. } => {
            assert!(text.starts_with("<image path=\""), "{text}");
            assert!(text.contains("shot.png"), "{text}");
        }
        other => panic!("expected an opening tag, got {other:?}"),
    }
    match &expanded.blocks[1] {
        ContentBlock::ImageUrl { image_url } => {
            assert!(image_url.url.starts_with("data:image/png;base64,"));
        }
        other => panic!("expected an image block, got {other:?}"),
    }
    assert_eq!(
        expanded.blocks[2],
        ContentBlock::Text {
            text: "</image>".to_string(),
            cache_control: None
        }
    );
}

#[test]
fn expands_multiple_placeholders_in_order() {
    let dir = tempfile::tempdir().expect("tempdir");
    let first = write_png(dir.path(), "one.png");
    let second = write_png(dir.path(), "two.png");
    std::fs::write(&second, [0xff, 0xd8, 0xff, 0xe0, 0x01]).expect("write jpeg");
    let text = format!(
        "[Attached image: {}]\nand\n[Attached image: {}]",
        first.display(),
        second.display()
    );

    let expanded = expand_attachment_blocks(&text);

    let media: Vec<_> = expanded
        .blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ImageUrl { image_url } => Some(
                parse_data_url(&image_url.url)
                    .expect("data url")
                    .0
                    .to_string(),
            ),
            _ => None,
        })
        .collect();
    assert_eq!(media, vec!["image/png", "image/jpeg"]);

    // Each image carries its own path tag, so the model can tell two
    // screenshots in one turn apart.
    let tags: Vec<_> = expanded
        .blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text, .. } if text.starts_with("<image path=") => {
                Some(text.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(tags.len(), 2, "{tags:?}");
    assert!(tags[0].contains("one.png"), "{tags:?}");
    assert!(tags[1].contains("two.png"), "{tags:?}");
}

#[test]
fn ingest_does_not_consult_model_capability() {
    // Capability is a route property and is re-decided per request. If
    // ingest started gating on it, attaching under a text-only model would
    // destroy the image for the rest of the session.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_png(dir.path(), "shot.png");
    let text = format!("[Attached image: {}]", path.display());

    let expanded = expand_attachment_blocks(&text);

    assert_eq!(expanded.blocks.len(), 3);
    assert!(expanded.notices.is_empty());
}

#[test]
fn a_blind_route_gets_text_in_place_of_every_image() {
    let mut messages = vec![
        message_with_image("data:image/png;base64,QUJD"),
        crate::models::Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: "sure".to_string(),
                cache_control: None,
            }],
        },
    ];

    let stripped =
        strip_images_when_unsupported(&mut messages, SupportState::Unsupported, "deepseek-chat");

    assert_eq!(stripped, 1);
    assert!(
        !messages[0]
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::ImageUrl { .. })),
        "no image may survive to a route that cannot read one"
    );
    match &messages[0].content[0] {
        ContentBlock::Text { text, .. } => {
            assert!(text.contains("deepseek-chat"), "{text}");
            assert!(text.contains("/model"), "{text}");
            assert!(text.contains("omitted"), "{text}");
        }
        other => panic!("expected replacement text, got {other:?}"),
    }
}

#[test]
fn a_supported_or_unknown_route_keeps_its_images() {
    // Unknown is the common case: models.dev has no modality data for most
    // routes. Stripping there would make the feature dead on arrival for
    // self-hosted and custom providers.
    for vision in [SupportState::Supported, SupportState::Unknown] {
        let mut messages = vec![message_with_image("data:image/png;base64,QUJD")];

        let stripped = strip_images_when_unsupported(&mut messages, vision, "some-model");

        assert_eq!(stripped, 0, "{vision:?} must not strip");
        assert!(
            messages[0]
                .content
                .iter()
                .any(|block| matches!(block, ContentBlock::ImageUrl { .. })),
            "{vision:?} must keep the image"
        );
    }
}

#[test]
fn stripping_replaces_every_image_across_every_message() {
    let mut messages = vec![
        message_with_image("data:image/png;base64,AAAA"),
        message_with_image("data:image/jpeg;base64,BBBB"),
    ];

    let stripped = strip_images_when_unsupported(&mut messages, SupportState::Unsupported, "blind");

    assert_eq!(
        stripped, 2,
        "a per-message early return would miss the second"
    );
}

#[test]
fn a_missing_file_becomes_a_notice_not_a_dropped_turn() {
    let text = "[Attached image: /nonexistent/definitely-not-here.png]";

    let expanded = expand_attachment_blocks(text);

    assert!(expanded.blocks.is_empty());
    assert_eq!(expanded.notices.len(), 1);
    assert!(
        expanded.notices[0].contains("definitely-not-here.png"),
        "{:?}",
        expanded.notices
    );
}

#[test]
fn one_bad_attachment_does_not_suppress_a_good_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let good = write_png(dir.path(), "good.png");
    let text = format!(
        "[Attached image: /nope/missing.png]\n[Attached image: {}]",
        good.display()
    );

    let expanded = expand_attachment_blocks(&text);

    assert_eq!(expanded.blocks.len(), 3);
    assert_eq!(expanded.notices.len(), 1);
}

#[test]
fn video_attachments_are_left_as_text() {
    let text = "[Attached video: /tmp/clip.mp4]";

    let expanded = expand_attachment_blocks(text);

    assert!(expanded.blocks.is_empty(), "{expanded:?}");
    assert!(expanded.notices.is_empty(), "{expanded:?}");
}

#[test]
fn text_with_no_attachments_produces_nothing() {
    let expanded = expand_attachment_blocks("just a normal question");
    assert!(expanded.blocks.is_empty());
    assert!(expanded.notices.is_empty());
}

#[test]
fn notice_block_names_the_failure_and_forbids_guessing() {
    assert_eq!(notice_block(&[]), None);
    let block =
        notice_block(&["Cannot attach a.png: the file is empty".to_string()]).expect("block");
    match block {
        ContentBlock::Text { text, .. } => {
            assert!(text.contains("<attachment_notice>"), "{text}");
            assert!(text.contains("a.png"), "{text}");
            assert!(text.contains("Do not describe"), "{text}");
        }
        other => panic!("expected text, got {other:?}"),
    }
}

#[test]
fn attach_from_path_reports_an_unreadable_file() {
    let error = attach_image_from_path(Path::new("/nonexistent/x.png")).expect_err("must fail");
    assert!(
        matches!(error, ImageAttachError::Unreadable { .. }),
        "got {error:?}"
    );
}
