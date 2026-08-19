use clap::error::ErrorKind;

use session::cli::{
    CliOutput,
    error_output,
    parse_cli_from,
    render_machine_output,
    requested_machine_output_format_from_args,
    run,
};

fn main() {
    let cli = match parse_cli_from(std::env::args_os()) {
        Ok(cli) => cli,
        Err(err) => {
            if matches!(
                err.kind(),
                ErrorKind::DisplayHelp
                    | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
                    | ErrorKind::DisplayVersion
            ) {
                print!("{err}");
                std::process::exit(0);
            }
            let rendered = error_output(
                &err.to_string(),
                requested_machine_output_format_from_args(),
            );
            eprintln!("{rendered}");
            std::process::exit(2);
        },
    };

    match run(cli) {
        Ok(CliOutput::Machine(value, format)) => {
            match render_machine_output(&value, format) {
                Ok(rendered) => println!("{rendered}"),
                Err(err) => {
                    eprintln!("{}", error_output(&err, Some(format)));
                    std::process::exit(1);
                },
            }
        },
        Ok(CliOutput::Text(text)) => println!("{text}"),
        Err(err) => {
            eprintln!(
                "{}",
                error_output(
                    &err.to_string(),
                    requested_machine_output_format_from_args(),
                )
            );
            std::process::exit(1);
        },
    }
}
