#[test]
fn command_labels_handle_missing_and_present_subcommands() {
    for (command, subcommand, label) in [
        (" \t ", "", ""),
        ("  ls  ", "", "ls"),
        ("\tgit\tstatus --short ", "status", "git status"),
    ] {
        assert_eq!(super::extract_subcmd(command), subcommand);
        assert_eq!(super::truncate_command(command), label);
    }
}
