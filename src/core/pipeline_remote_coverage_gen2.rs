use super::urlencode;

#[test]
fn urlencode_preserves_literal_pluses_and_hashes_in_search_terms() {
    assert_eq!(
        urlencode("topic:rtk-pipeline C++ & C#"),
        "topic:rtk-pipeline%20C%2B%2B%20%26%20C%23"
    );
}
