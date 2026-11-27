use super::*;

#[test]
fn scores_use_the_configured_floor_and_accept_zero_confidence_by_default() {
    for (floor, confidence, expected) in [
        (Some(0.75), 0.5, None),
        (Some(0.75), 0.75, Some(Fact::Num(0.375))),
        (None, 0.0, Some(Fact::Num(0.375))),
    ] {
        let def = ClassifierDef {
            question: "score".into(),
            min_confidence: floor,
            ..ClassifierDef::default()
        };
        let mut facts = Facts::new();

        apply_answer(
            "quality",
            &def,
            &Answer::Score {
                score: 0.375,
                confidence,
            },
            &mut facts,
        );

        assert_eq!(
            facts.get("score.quality"),
            expected.as_ref(),
            "confidence {confidence} with floor {floor:?}",
        );
    }
}

#[test]
fn noul_answers_set_numeric_facts_even_at_zero() {
    let def = ClassifierDef {
        question: "noul".into(),
        min_confidence: Some(1.0),
        ..ClassifierDef::default()
    };
    let mut facts = Facts::new();

    for value in [0.625, 0.0] {
        apply_answer("relevance", &def, &Answer::Noul { value }, &mut facts);

        assert_eq!(facts.get("noul.relevance"), Some(&Fact::Num(value)));
    }
}

#[test]
fn cache_decoder_rejects_unknown_types_with_otherwise_valid_fields() {
    let json = r#"{"type":"future","choice":"build_log","confidence":1.0}"#;

    assert_eq!(answer_from_json(json), None);
}
