use structopt::StructOpt;

#[derive(StructOpt, Debug)]
pub struct Opt {
    // Programs to execute, instead of reading 'stdin'. These are separated by '-/-'.
    pub programs: Vec<String>,

    // Regex to match context beginning
    #[structopt(short = "-s", long = "match-begin")]
    pub match_start: Vec<String>,

    // Regex to match context end
    #[structopt(short = "-e", long = "match-end")]
    pub match_end: Vec<String>,

    // Load additional Regex pairs from given file, one pair per two lines.
    #[structopt(short = "-f", long = "match-pairs-file")]
    pub match_pairs_file: Option<String>,

    // Instead of stdin, describe shell programs to from given input file
    // a shell script per line. If '-' then reads shell scripts from stdin.
    #[structopt(short = "-p", long = "programs-file")]
    pub programs_file: Option<String>,

    // Use provided shell executable rather than the default `/bin/sh`.
    #[structopt(short = "-C", long = "shell")]
    pub shell: Option<String>,

    // Work in an alternative screen, and dump the original input after we are done
    // processing.
    #[structopt(short = "-r", long = "replay")]
    pub replay: bool,

    // Amount of lines to remove from final report size, so that the prompt being
    // printed afterward will fit.
    #[structopt(short = "-x", long = "final-shrink", default_value = "2")]
    pub final_shrink: usize,

    // Interline delay for demoing purposes
    #[structopt(short = "-D", long = "interline-delay", default_value = "0")]
    pub interline_delay: usize,

    #[structopt(short = "-d", long = "debug")]
    pub debug: bool,
}
