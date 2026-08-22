use sz_rust_ai_facade::llm::provider::{ChatMessage, ContentPart, ImageDetail, Role};

#[test]
fn text_content_part_serializes_as_string() {
    let part = ContentPart::Text("hello world".to_string());
    let json = serde_json::to_value(&part).unwrap();
    assert_eq!(json, serde_json::Value::String("hello world".to_string()));
}

#[test]
fn text_content_part_deserializes_from_string() {
    let json = serde_json::Value::String("hello world".to_string());
    let part: ContentPart = serde_json::from_value(json).unwrap();
    assert_eq!(part, ContentPart::Text("hello world".to_string()));
    assert_eq!(part.as_text(), Some("hello world"));
}

#[test]
fn image_content_part_serializes_as_object() {
    let part = ContentPart::Image {
        url: "https://example.com/img.png".to_string(),
        detail: ImageDetail::High,
    };
    let json = serde_json::to_value(&part).unwrap();
    assert!(json.is_object());
    assert_eq!(json["url"], "https://example.com/img.png");
    assert_eq!(json["detail"], "high");
}

#[test]
fn image_content_part_deserializes_from_object() {
    let json = serde_json::json!({
        "url": "https://example.com/img.png",
        "detail": "low"
    });
    let part: ContentPart = serde_json::from_value(json).unwrap();
    match part {
        ContentPart::Image { url, detail } => {
            assert_eq!(url, "https://example.com/img.png");
            assert_eq!(detail, ImageDetail::Low);
        }
        _ => panic!("expected Image variant"),
    }
}

#[test]
fn image_base64_content_part_serializes_as_object() {
    let part = ContentPart::ImageBase64 {
        data: "iVBORw0KGgo=".to_string(),
        mime_type: "image/png".to_string(),
    };
    let json = serde_json::to_value(&part).unwrap();
    assert!(json.is_object());
    assert_eq!(json["data"], "iVBORw0KGgo=");
    assert_eq!(json["mime_type"], "image/png");
}

#[test]
fn image_base64_content_part_deserializes_from_object() {
    let json = serde_json::json!({
        "data": "iVBORw0KGgo=",
        "mime_type": "image/jpeg"
    });
    let part: ContentPart = serde_json::from_value(json).unwrap();
    match part {
        ContentPart::ImageBase64 { data, mime_type } => {
            assert_eq!(data, "iVBORw0KGgo=");
            assert_eq!(mime_type, "image/jpeg");
        }
        _ => panic!("expected ImageBase64 variant"),
    }
}

#[test]
fn image_detail_serialization_all_variants() {
    assert_eq!(
        serde_json::to_string(&ImageDetail::Low).unwrap(),
        r#""low""#
    );
    assert_eq!(
        serde_json::to_string(&ImageDetail::High).unwrap(),
        r#""high""#
    );
    assert_eq!(
        serde_json::to_string(&ImageDetail::Auto).unwrap(),
        r#""auto""#
    );
}

#[test]
fn image_detail_deserialization_all_variants() {
    let low: ImageDetail = serde_json::from_str(r#""low""#).unwrap();
    let high: ImageDetail = serde_json::from_str(r#""high""#).unwrap();
    let auto: ImageDetail = serde_json::from_str(r#""auto""#).unwrap();
    assert_eq!(low, ImageDetail::Low);
    assert_eq!(high, ImageDetail::High);
    assert_eq!(auto, ImageDetail::Auto);
}

#[test]
fn chat_message_with_text_content_backward_compat() {
    let old_json = r#"{
        "role": "user",
        "content": "hello world",
        "tool_call_id": null,
        "tool_calls": null
    }"#;
    let msg: ChatMessage = serde_json::from_str(old_json).unwrap();
    assert_eq!(msg.role, Role::User);
    assert_eq!(msg.content.as_text(), Some("hello world"));
    assert!(msg.tool_call_id.is_none());
    assert!(msg.tool_calls.is_none());
}

#[test]
fn chat_message_with_image_content_roundtrip() {
    let msg = ChatMessage {
        role: Role::User,
        content: ContentPart::Image {
            url: "https://example.com/photo.jpg".to_string(),
            detail: ImageDetail::Auto,
        },
        tool_call_id: None,
        tool_calls: None,
    };
    let json = serde_json::to_string(&msg).unwrap();
    let deserialized: ChatMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(msg.role, deserialized.role);
    assert_eq!(msg.content, deserialized.content);
    assert!(msg.tool_call_id.is_none());
    assert!(deserialized.tool_call_id.is_none());
    assert!(msg.tool_calls.is_none());
    assert!(deserialized.tool_calls.is_none());
}

#[test]
fn chat_message_with_image_base64_content_roundtrip() {
    let msg = ChatMessage {
        role: Role::User,
        content: ContentPart::ImageBase64 {
            data: "base64data".to_string(),
            mime_type: "image/gif".to_string(),
        },
        tool_call_id: None,
        tool_calls: None,
    };
    let json = serde_json::to_string(&msg).unwrap();
    let deserialized: ChatMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(msg.role, deserialized.role);
    assert_eq!(msg.content, deserialized.content);
    assert!(msg.tool_call_id.is_none());
    assert!(deserialized.tool_call_id.is_none());
    assert!(msg.tool_calls.is_none());
    assert!(deserialized.tool_calls.is_none());
}

#[test]
fn content_part_from_string_conversion() {
    let part: ContentPart = "hello".into();
    assert_eq!(part, ContentPart::Text("hello".to_string()));

    let part2: ContentPart = String::from("world").into();
    assert_eq!(part2, ContentPart::Text("world".to_string()));
}

#[test]
fn content_part_display_implementation() {
    let text = ContentPart::Text("hello".to_string());
    assert_eq!(format!("{text}"), "hello");

    let img = ContentPart::Image {
        url: "https://example.com/img.png".to_string(),
        detail: ImageDetail::Low,
    };
    assert_eq!(format!("{img}"), "[image:https://example.com/img.png]");

    let img64 = ContentPart::ImageBase64 {
        data: "abc".to_string(),
        mime_type: "image/png".to_string(),
    };
    assert_eq!(format!("{img64}"), "[image:image/png]");
}

#[test]
fn content_part_text_or_empty_method() {
    let text = ContentPart::Text("hello".to_string());
    assert_eq!(text.text_or_empty(), "hello");

    let img = ContentPart::Image {
        url: "https://example.com/img.png".to_string(),
        detail: ImageDetail::Low,
    };
    assert_eq!(img.text_or_empty(), "");

    let empty_text = ContentPart::Text(String::new());
    assert_eq!(empty_text.text_or_empty(), "");
}

#[test]
fn content_part_is_image_method() {
    let text = ContentPart::Text("hello".to_string());
    assert!(!text.is_image());

    let img = ContentPart::Image {
        url: "https://example.com/img.png".to_string(),
        detail: ImageDetail::Low,
    };
    assert!(img.is_image());

    let img64 = ContentPart::ImageBase64 {
        data: "abc".to_string(),
        mime_type: "image/png".to_string(),
    };
    assert!(img64.is_image());
}

#[test]
fn content_part_default_is_empty_text() {
    let part = ContentPart::default();
    assert_eq!(part, ContentPart::Text(String::new()));
    assert_eq!(part.text_or_empty(), "");
}

#[test]
fn content_part_clone_and_equality() {
    let part1 = ContentPart::Text("hello".to_string());
    let part2 = part1.clone();
    assert_eq!(part1, part2);

    let img1 = ContentPart::Image {
        url: "https://example.com/img.png".to_string(),
        detail: ImageDetail::High,
    };
    let img2 = img1.clone();
    assert_eq!(img1, img2);

    let img3 = ContentPart::Image {
        url: "https://example.com/img.png".to_string(),
        detail: ImageDetail::Low,
    };
    assert_ne!(img1, img3);
}

#[test]
fn empty_string_content_part_serializes_as_empty_string() {
    let part = ContentPart::Text(String::new());
    let json = serde_json::to_string(&part).unwrap();
    assert_eq!(json, r#""""#);
}
