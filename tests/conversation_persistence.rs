use dragoman::session::SessionData;
use dragoman::web::GeminiWebClient;
use serde_json::json;

#[test]
fn test_stream_generate_payload_slot_defaults() {
    let payload = GeminiWebClient::build_stream_generate_payload("Hello world", None, None);

    assert_eq!(payload.len(), 99);
    assert_eq!(
        payload[0],
        json!(["Hello world", 0, null, null, null, null, 0])
    );
    assert_eq!(payload[1], json!(["en"]));
    // Slot 3 (response_id) must be Null on initial turn
    assert!(payload[3].is_null());
    // Slot 17 must enforce Argon
    assert_eq!(payload[17], json!([[2]]));
    // Slot 71 (conversation_id) must be Null on initial turn
    assert!(payload[71].is_null());
}

#[test]
fn test_stream_generate_payload_multi_turn_slots() {
    let conv_id = "c_conv_42";
    let resp_id = "r_resp_99";
    let payload = GeminiWebClient::build_stream_generate_payload(
        "Next question",
        Some(conv_id),
        Some(resp_id),
    );

    assert_eq!(payload.len(), 99);
    assert_eq!(
        payload[0],
        json!(["Next question", 0, null, null, null, null, 0])
    );
    // Slot 3 must contain response_id
    assert_eq!(payload[3], json!(resp_id));
    // Slot 71 must contain array with conversation_id at index 0
    assert_eq!(
        payload[71],
        json!([conv_id, null, null, null, null, null, null, null])
    );
}

#[test]
fn test_form_envelope_serialization() {
    let conv_id = "c_test_conv";
    let resp_id = "r_test_resp";
    let envelope =
        GeminiWebClient::build_form_envelope("Test prompt", Some(conv_id), Some(resp_id));

    let parsed: serde_json::Value =
        serde_json::from_str(&envelope).expect("Envelope must be valid JSON array");
    let arr = parsed.as_array().expect("Envelope must be array");
    assert_eq!(arr.len(), 2);
    assert!(arr[0].is_null());

    let inner_str = arr[1]
        .as_str()
        .expect("Envelope second element must be inner JSON string");
    let inner: Vec<serde_json::Value> =
        serde_json::from_str(inner_str).expect("Inner string must be valid JSON array");
    assert_eq!(inner.len(), 99);
    assert_eq!(inner[3], json!(resp_id));
    assert_eq!(
        inner[71],
        json!([conv_id, null, null, null, null, null, null, null])
    );
}

#[test]
fn test_wrb_fr_parsing_metadata_and_candidate() {
    let line = r#"[[["wrb.fr", null, "[\"ignored\", [\"c_session_001\", \"r_turn_001\"], null, null, [[null, [\"Gemini Argon output text\"]]]]"]]]"#;

    let parsed = GeminiWebClient::parse_wrb_fr_line(line);
    assert!(parsed.is_some());

    let payload = parsed.unwrap();
    assert_eq!(payload.conversation_id, Some("c_session_001".to_string()));
    assert_eq!(payload.response_id, Some("r_turn_001".to_string()));
    assert_eq!(
        payload.candidate_text,
        Some("Gemini Argon output text".to_string())
    );
}

#[test]
fn test_wrb_fr_parsing_with_choice_id() {
    let line = r#"[[["wrb.fr", null, "[\"ignored\", [\"c_session_002\", \"r_turn_002\"], null, null, [[\"rc_choice_999\", [\"Candidate with choice id\"]]]]"]]]"#;

    let parsed = GeminiWebClient::parse_wrb_fr_line(line);
    assert!(parsed.is_some());

    let payload = parsed.unwrap();
    assert_eq!(payload.conversation_id, Some("c_session_002".to_string()));
    assert_eq!(payload.response_id, Some("r_turn_002".to_string()));
    assert_eq!(payload.choice_id, Some("rc_choice_999".to_string()));
    assert_eq!(
        payload.candidate_text,
        Some("Candidate with choice id".to_string())
    );
}

#[test]
fn test_wrb_fr_parsing_with_inline_xssi_prefix() {
    let line = r#")]}'[[["wrb.fr", null, "[\"ignored\", [\"c_xssi\", \"r_xssi\"], null, null, [[null, [\"Parsed despite XSSI preamble\"]]]]"]]]"#;

    let parsed = GeminiWebClient::parse_wrb_fr_line(line);
    assert!(parsed.is_some());

    let payload = parsed.unwrap();
    assert_eq!(payload.conversation_id, Some("c_xssi".to_string()));
    assert_eq!(payload.response_id, Some("r_xssi".to_string()));
    assert_eq!(
        payload.candidate_text,
        Some("Parsed despite XSSI preamble".to_string())
    );
}

#[test]
fn test_wrb_fr_parsing_empty_candidates() {
    let line =
        r#"[[["wrb.fr", null, "[\"ignored\", [\"c_empty\", \"r_empty\"], null, null, []]"]]]"#;

    let parsed = GeminiWebClient::parse_wrb_fr_line(line);
    assert!(parsed.is_some());

    let payload = parsed.unwrap();
    assert_eq!(payload.conversation_id, Some("c_empty".to_string()));
    assert_eq!(payload.response_id, Some("r_empty".to_string()));
    assert!(payload.candidate_text.is_none());
    assert!(payload.choice_id.is_none());
}

#[test]
fn test_wrb_fr_parsing_preambles_and_noise() {
    assert!(GeminiWebClient::parse_wrb_fr_line(")]}'").is_none());
    assert!(GeminiWebClient::parse_wrb_fr_line("12345").is_none());
    assert!(GeminiWebClient::parse_wrb_fr_line("").is_none());
    assert!(GeminiWebClient::parse_wrb_fr_line("   \n").is_none());
    assert!(GeminiWebClient::parse_wrb_fr_line("invalid json content").is_none());
}

#[tokio::test]
async fn test_multi_turn_session_chaining_simulation() {
    let mut session = SessionData {
        snlm0e: "test_snlm0e".to_string(),
        f_sid: "test_f_sid".to_string(),
        bl: "test_bl".to_string(),
        cookie: "test_cookie".to_string(),
        model: "Argon4p0".to_string(),
        conversation_id: None,
        response_id: None,
        choice_id: None,
    };

    // --- Turn 1 ---
    let t1_payload = GeminiWebClient::build_stream_generate_payload(
        "Who was the first president of the United States?",
        session.conversation_id.as_deref(),
        session.response_id.as_deref(),
    );
    assert!(t1_payload[71].is_null());
    assert!(t1_payload[3].is_null());

    // Upstream returns Turn 1 response with established conversation and response ID
    let t1_upstream_chunk = r#"[[["wrb.fr", null, "[\"wrb.fr\", [\"c_us_history\", \"r_turn1_resp\"], null, null, [[\"rc_turn1_choice\", [\"George Washington.\"]]]]"]]]"#;
    let t1_res = GeminiWebClient::parse_wrb_fr_line(t1_upstream_chunk)
        .expect("Turn 1 upstream chunk must parse");
    assert_eq!(
        t1_res.candidate_text,
        Some("George Washington.".to_string())
    );

    // Session update after Turn 1
    session.conversation_id = t1_res.conversation_id;
    session.response_id = t1_res.response_id;
    session.choice_id = t1_res.choice_id;

    assert_eq!(session.conversation_id.as_deref(), Some("c_us_history"));
    assert_eq!(session.response_id.as_deref(), Some("r_turn1_resp"));
    assert_eq!(session.choice_id.as_deref(), Some("rc_turn1_choice"));

    // --- Turn 2 ---
    let t2_payload = GeminiWebClient::build_stream_generate_payload(
        "When was he born?",
        session.conversation_id.as_deref(),
        session.response_id.as_deref(),
    );
    // Verify slot 71 chains the conversation ID
    assert_eq!(
        t2_payload[71],
        json!(["c_us_history", null, null, null, null, null, null, null])
    );
    // Verify slot 3 chains the previous response ID
    assert_eq!(t2_payload[3], json!("r_turn1_resp"));

    // Upstream returns Turn 2 response, continuing the conversation with a new response ID
    let t2_upstream_chunk = r#"[[["wrb.fr", null, "[\"wrb.fr\", [\"c_us_history\", \"r_turn2_resp\"], null, null, [[\"rc_turn2_choice\", [\"He was born on February 22, 1732.\"]]]]"]]]"#;
    let t2_res = GeminiWebClient::parse_wrb_fr_line(t2_upstream_chunk)
        .expect("Turn 2 upstream chunk must parse");
    assert_eq!(
        t2_res.candidate_text,
        Some("He was born on February 22, 1732.".to_string())
    );

    // Session update after Turn 2
    if let Some(cid) = t2_res.conversation_id {
        session.conversation_id = Some(cid);
    }
    if let Some(rid) = t2_res.response_id {
        session.response_id = Some(rid);
    }
    if let Some(chid) = t2_res.choice_id {
        session.choice_id = Some(chid);
    }

    assert_eq!(session.conversation_id.as_deref(), Some("c_us_history"));
    assert_eq!(session.response_id.as_deref(), Some("r_turn2_resp"));
    assert_eq!(session.choice_id.as_deref(), Some("rc_turn2_choice"));

    // --- Turn 3 ---
    let t3_payload = GeminiWebClient::build_stream_generate_payload(
        "Where did he live?",
        session.conversation_id.as_deref(),
        session.response_id.as_deref(),
    );
    assert_eq!(
        t3_payload[71],
        json!(["c_us_history", null, null, null, null, null, null, null])
    );
    assert_eq!(t3_payload[3], json!("r_turn2_resp"));
}

#[test]
fn test_session_serialization_backward_compatibility() {
    // Legacy JSON without conversation_id and response_id
    let legacy_json = r#"{
        "snlm0e": "legacy_at",
        "f_sid": "legacy_fsid",
        "bl": "legacy_bl",
        "cookie": "legacy_cookie"
    }"#;

    let session: SessionData =
        serde_json::from_str(legacy_json).expect("Legacy session JSON must deserialize cleanly");
    assert_eq!(session.model, "Argon4p0");
    assert!(session.conversation_id.is_none());
    assert!(session.response_id.is_none());

    // Roundtrip with populated IDs
    let full_session = SessionData {
        snlm0e: "at_123".to_string(),
        f_sid: "fsid_123".to_string(),
        bl: "bl_123".to_string(),
        cookie: "cookie_123".to_string(),
        model: "Argon4p0".to_string(),
        conversation_id: Some("c_conv_abc".to_string()),
        response_id: Some("r_resp_xyz".to_string()),
        choice_id: Some("rc_001".to_string()),
    };

    let serialized = serde_json::to_string(&full_session).unwrap();
    let deserialized: SessionData = serde_json::from_str(&serialized).unwrap();
    assert_eq!(deserialized.conversation_id.as_deref(), Some("c_conv_abc"));
    assert_eq!(deserialized.response_id.as_deref(), Some("r_resp_xyz"));
    assert_eq!(deserialized.choice_id.as_deref(), Some("rc_001"));
}

#[test]
fn test_delta_char_boundary_utf8_vietnamese() {
    let prev = "Xin chào các bạn";
    let next_extend = "Xin chào các bạn, chúc một ngày tốt lành!";
    assert!(next_extend.starts_with(prev));
    let delta = &next_extend[prev.len()..];
    assert_eq!(delta, ", chúc một ngày tốt lành!");

    // Non-prefix replacement or correction with multi-byte characters
    let prev2 = "Xin chào các bạn";
    let next2 = "Xin chào mọi người";
    let common_len = prev2
        .chars()
        .zip(next2.chars())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum::<usize>();
    assert_eq!(&prev2[..common_len], "Xin chào ");
    let delta2 = &next2[common_len..];
    assert_eq!(delta2, "mọi người");
}
