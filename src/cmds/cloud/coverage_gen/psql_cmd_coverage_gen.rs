use super::filter_psql_output;

#[test]
fn table_output_omits_blank_lines() {
    let input = concat!(
        " id | label \n",
        "----+-------\n",
        "\n",
        "  7 | first \n",
        " \t \n",
        "  8 | second \n",
        "(2 rows)\n",
    );

    assert_eq!(filter_psql_output(input), "id\tlabel\n7\tfirst\n8\tsecond");
}

#[test]
fn expanded_output_keeps_the_notice_but_omits_blank_lines() {
    let input = concat!(
        "  NOTICE: showing sample rows  \n",
        " \t \n",
        "-[ RECORD 1 ]------\n",
        "id    | 7\n",
        "\n",
        "label | left | right\n",
        "-[ RECORD 2 ]------\n",
        "id    | 8\n",
        "(2 rows)\n",
    );

    assert_eq!(
        filter_psql_output(input),
        "NOTICE: showing sample rows\n[1] id=7 label=left | right\n[2] id=8"
    );
}
