use super::*;

#[test]
fn preserves_misaligned_data_url_but_elides_padded_neighbor() {
    // Both runs exceed the threshold and contain upper- and lowercase letters
    // and digits. Only the second run is padded to a multiple of four.
    let body = format!("{}Aa0", "Aa0A".repeat(MIN_RUN / 4));
    let unpadded = format!("data:image/png;base64,{body}");
    let input = format!(r#"{{"unpadded":"{unpadded}","padded":"{unpadded}="}}"#);
    let block = crate::testutil::block(&input, crate::block::ContentFormat::Json);

    let Outcome::Continue(output) = Base64Elide.apply(&block, &MethodCtx::new("test.json")) else {
        panic!("base64 elision should continue the pipeline");
    };

    assert_eq!(
        output,
        format!(r#"{{"unpadded":"{unpadded}","padded":"[base64 image/png, 1 KB elided]"}}"#)
    );
}
