use super::{Code, Diagnostic, Severity};

#[test]
fn numbered_codes_keep_their_labels_and_severities() {
    for (code, label) in [
        (Code::E006, "E006"),
        (Code::E007, "E007"),
        (Code::E008, "E008"),
        (Code::E009, "E009"),
        (Code::E010, "E010"),
        (Code::E011, "E011"),
        (Code::E012, "E012"),
        (Code::E013, "E013"),
        (Code::E015, "E015"),
        (Code::E016, "E016"),
        (Code::E017, "E017"),
        (Code::E018, "E018"),
        (Code::E019, "E019"),
        (Code::E020, "E020"),
    ] {
        assert_eq!(code.as_str(), label);
        assert_eq!(code.severity(), Severity::Error, "{label}");
    }

    for (code, label) in [
        (Code::W002, "W002"),
        (Code::W003, "W003"),
        (Code::W004, "W004"),
        (Code::W005, "W005"),
        (Code::W008, "W008"),
        (Code::W018, "W018"),
        (Code::W019, "W019"),
        (Code::W024, "W024"),
    ] {
        assert_eq!(code.as_str(), label);
        assert_eq!(code.severity(), Severity::Warning, "{label}");
    }
}

#[test]
fn trust_warning_separates_context_from_a_present_filename() {
    let diagnostic = Diagnostic::new(Code::Trust, "pipeline is not trusted").context("[pipeline]");
    assert_eq!(diagnostic.severity(), Severity::Warning);
    assert_eq!(
        diagnostic.render(),
        "[pipeline]\n  TRUST  pipeline is not trusted"
    );

    let diagnostic = diagnostic.at("pipeline.toml", None);
    assert_eq!(
        diagnostic.render(),
        "pipeline.toml  [pipeline]\n  TRUST  pipeline is not trusted"
    );
}
