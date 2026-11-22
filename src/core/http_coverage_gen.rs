use super::{Uri, is_local};

#[test]
fn localhost_in_a_path_is_not_a_local_host() {
    let uri = "/localhost/health"
        .parse::<Uri>()
        .expect("valid path-only URI");
    assert!(
        !is_local(&uri),
        "a URI without a host must not be classified as local"
    );
}
