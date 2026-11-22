use super::generics_len;

#[test]
fn unclosed_generic_list_consumes_the_remaining_bytes() {
    let code = "<Vec<Café>, usize";

    assert_eq!(generics_len(code), code.len());
}
