//! CLI logic for `rccx`.
//!
//! The `main.rs` binary is a thin wrapper around [`run`], which is exposed
//! as a library entry point so it can be exercised from tests.
//!
//! Output is captured through [`Streams`]: production builds wire it to
//! real stdout/stderr, while tests use in-memory buffers.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use rccx_diagnostics::code::E_BAD_CLI;
use rccx_diagnostics::{render::render_codeless, Diagnostic};
use rccx_driver::{
    options::{CStandard, EmitKind, Options, SafeCMode, UserDefine},
    HELP_TEXT, VERSION,
};

/// Stdout/stderr the CLI writes to. Production code uses real streams;
/// tests use in-memory `Vec<u8>`.
pub struct Streams<'a> {
    pub stdout: &'a mut dyn Write,
    pub stderr: &'a mut dyn Write,
}

/// Run with real stdout/stderr. Returns the process exit code.
pub fn run(args: &[String]) -> ExitCode {
    let mut out = std::io::stdout().lock();
    let mut err = std::io::stderr().lock();
    let mut streams = Streams {
        stdout: &mut out,
        stderr: &mut err,
    };
    code_to_exit(run_with(args, &mut streams))
}

/// Test-friendly entry: same logic as [`run`], but takes explicit streams.
pub fn run_with(args: &[String], streams: &mut Streams<'_>) -> u8 {
    match parse_args(args) {
        Ok(Action::PrintVersion) => {
            let _ = writeln!(streams.stdout, "rccx {VERSION}");
            0
        }
        Ok(Action::PrintHelp) => {
            let _ = writeln!(streams.stdout, "{HELP_TEXT}");
            0
        }
        Ok(Action::Explain(code)) => match rccx_driver::explain(&code) {
            Ok(text) => {
                let _ = writeln!(streams.stdout, "{text}");
                0
            }
            Err(diag) => {
                let _ = write!(streams.stderr, "{}", render_codeless(&diag));
                1
            }
        },
        Ok(Action::Compile(opts)) => {
            let result = rccx_driver::compile(&opts);
            if !result.emit.is_empty() {
                let _ = write!(streams.stdout, "{}", result.emit);
            }
            for diag in &result.diagnostics {
                let _ = write!(
                    streams.stderr,
                    "{}",
                    rccx_diagnostics::render_human(diag, &result.sources)
                );
            }
            if result.success {
                0
            } else {
                1
            }
        }
        Err(diag) => {
            let _ = write!(streams.stderr, "{}", render_codeless(&diag));
            let _ = writeln!(streams.stderr, "For usage, run `rccx --help`.");
            2
        }
    }
}

fn code_to_exit(code: u8) -> ExitCode {
    ExitCode::from(code)
}

#[derive(Debug)]
enum Action {
    PrintVersion,
    PrintHelp,
    Explain(String),
    Compile(Options),
}

fn parse_args(args: &[String]) -> Result<Action, Diagnostic> {
    if args.is_empty() {
        // No args: show help on stdout so users can pipe it.
        return Ok(Action::PrintHelp);
    }

    let mut opts = Options::default();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "-h" | "--help" => return Ok(Action::PrintHelp),
            "-V" | "--version" => return Ok(Action::PrintVersion),
            "--explain" => {
                let code = args.get(i + 1).ok_or_else(|| {
                    Diagnostic::error(E_BAD_CLI, "`--explain` requires a diagnostic code")
                })?;
                return Ok(Action::Explain(code.clone()));
            }
            "--json-diagnostics" => {
                opts.json_diagnostics = true;
                i += 1;
            }
            "-fsafe-c" => {
                opts.safe_c = SafeCMode::Error;
                i += 1;
            }
            "-fno-safe-c" => {
                opts.safe_c = SafeCMode::Off;
                i += 1;
            }
            "-fsafe-c=warn" => {
                opts.safe_c = SafeCMode::Warn;
                i += 1;
            }
            "-o" => {
                let path = args
                    .get(i + 1)
                    .ok_or_else(|| Diagnostic::error(E_BAD_CLI, "`-o` requires a path argument"))?;
                opts.output = Some(PathBuf::from(path));
                i += 2;
            }
            "-I" => {
                let path = args
                    .get(i + 1)
                    .ok_or_else(|| Diagnostic::error(E_BAD_CLI, "`-I` requires a path argument"))?;
                opts.include_paths.push(PathBuf::from(path));
                i += 2;
            }
            "-D" => {
                let raw = args
                    .get(i + 1)
                    .ok_or_else(|| Diagnostic::error(E_BAD_CLI, "`-D` requires NAME[=BODY]"))?;
                opts.user_defines.push(parse_define(raw)?);
                i += 2;
            }
            _ if arg.starts_with("-I") && arg.len() > 2 => {
                opts.include_paths.push(PathBuf::from(&arg[2..]));
                i += 1;
            }
            _ if arg.starts_with("-D") && arg.len() > 2 => {
                opts.user_defines.push(parse_define(&arg[2..])?);
                i += 1;
            }
            _ if arg.starts_with("-std=") => {
                let val = &arg[5..];
                opts.standard = val
                    .parse::<CStandard>()
                    .map_err(|e| Diagnostic::error(E_BAD_CLI, e))?;
                i += 1;
            }
            _ if arg.starts_with("-emit=") => {
                let val = &arg[6..];
                opts.emit = Some(
                    val.parse::<EmitKind>()
                        .map_err(|e| Diagnostic::error(E_BAD_CLI, e))?,
                );
                i += 1;
            }
            _ if arg.starts_with("--") || arg.starts_with('-') => {
                return Err(Diagnostic::error(
                    E_BAD_CLI,
                    format!("unknown option `{arg}`"),
                ));
            }
            _ => {
                opts.inputs.push(PathBuf::from(arg));
                i += 1;
            }
        }
    }

    Ok(Action::Compile(opts))
}

fn parse_define(raw: &str) -> Result<UserDefine, Diagnostic> {
    let (name, body) = match raw.find('=') {
        Some(idx) => (&raw[..idx], &raw[idx + 1..]),
        None => (raw, "1"),
    };
    if name.is_empty() {
        return Err(Diagnostic::error(
            E_BAD_CLI,
            "`-D` requires a macro name before `=`",
        ));
    }
    if !is_valid_ident(name) {
        return Err(Diagnostic::error(
            E_BAD_CLI,
            format!("`-D` macro name `{name}` is not a valid identifier"),
        ));
    }
    Ok(UserDefine {
        name: name.to_string(),
        body: body.to_string(),
    })
}

fn is_valid_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_args(args: &[&str]) -> (u8, String, String) {
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut streams = Streams {
            stdout: &mut out,
            stderr: &mut err,
        };
        let code = run_with(&owned, &mut streams);
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn version_flag_prints_version() {
        let (code, out, err) = run_args(&["--version"]);
        assert_eq!(code, 0);
        assert!(out.starts_with("rccx "), "got: {out:?}");
        assert!(err.is_empty(), "got: {err:?}");
    }

    #[test]
    fn short_version_flag_works() {
        let (code, out, _) = run_args(&["-V"]);
        assert_eq!(code, 0);
        assert!(out.contains("rccx "));
    }

    #[test]
    fn help_flag_prints_usage() {
        let (code, out, _) = run_args(&["--help"]);
        assert_eq!(code, 0);
        assert!(out.contains("USAGE:"), "got: {out:?}");
        assert!(out.contains("Safe C"));
    }

    #[test]
    fn no_args_prints_help() {
        let (code, out, _) = run_args(&[]);
        assert_eq!(code, 0);
        assert!(out.contains("USAGE:"));
    }

    #[test]
    fn explain_known_code() {
        let (code, out, _) = run_args(&["--explain", "E0001"]);
        assert_eq!(code, 0);
        assert!(out.contains("use of moved owner pointer"));
    }

    #[test]
    fn explain_unknown_code_errors() {
        let (code, _, err) = run_args(&["--explain", "E1234"]);
        assert_eq!(code, 1);
        assert!(err.contains("unknown diagnostic code"));
    }

    #[test]
    fn explain_without_code_errors() {
        let (code, _, err) = run_args(&["--explain"]);
        assert_eq!(code, 2);
        assert!(err.contains("--explain"));
    }

    #[test]
    fn unknown_flag_errors() {
        let (code, _, err) = run_args(&["--definitely-not-real"]);
        assert_eq!(code, 2);
        assert!(err.contains("unknown option"));
    }

    #[test]
    fn no_input_compile_emits_diagnostic() {
        // Pass a flag that puts us into compile mode without inputs.
        let (code, _, err) = run_args(&["-fsafe-c"]);
        assert_eq!(code, 1);
        assert!(err.contains("E9003"), "got: {err:?}");
    }

    #[test]
    fn missing_input_file_reports_io() {
        let (code, _, err) = run_args(&["/no/such/file.c"]);
        assert_eq!(code, 1);
        assert!(err.contains("E9001"), "got: {err:?}");
    }

    #[test]
    fn unknown_std_errors() {
        let (code, _, err) = run_args(&["-std=c42", "a.c"]);
        assert_eq!(code, 2);
        assert!(err.contains("unknown C standard"));
    }

    #[test]
    fn unknown_emit_errors() {
        let (code, _, err) = run_args(&["-emit=nonsense", "a.c"]);
        assert_eq!(code, 2);
        assert!(err.contains("unknown -emit target"));
    }

    #[test]
    fn emit_tokens_writes_dump_to_stdout() {
        let dir = tmp_dir("emit-tokens");
        let path = dir.join("hello.c");
        std::fs::write(&path, "int x = 1;\n").unwrap();
        let path_str = path.to_string_lossy().to_string();
        let (code, out, err) = run_args(&["-emit=tokens", &path_str]);
        assert_eq!(code, 0, "stderr: {err}");
        assert!(out.contains("Keyword(int)"), "stdout: {out}");
        assert!(out.contains("EOF"), "stdout: {out}");
        assert!(err.is_empty(), "stderr: {err}");
    }

    #[test]
    fn dash_d_predefines_a_macro() {
        let dir = tmp_dir("dash-d");
        let path = dir.join("a.c");
        std::fs::write(&path, "int x = N;\n").unwrap();
        let path_str = path.to_string_lossy().to_string();
        let (code, out, err) = run_args(&["-DN=9", "-emit=pp-tokens", &path_str]);
        assert_eq!(code, 0, "stderr: {err}");
        assert!(out.contains("IntLiteral(Decimal) \"9\""), "stdout: {out}");
    }

    #[test]
    fn dash_d_without_body_defaults_to_one() {
        let dir = tmp_dir("dash-d-empty");
        let path = dir.join("a.c");
        std::fs::write(&path, "int x = N;\n").unwrap();
        let path_str = path.to_string_lossy().to_string();
        let (code, out, _) = run_args(&["-D", "N", "-emit=pp-tokens", &path_str]);
        assert_eq!(code, 0);
        assert!(out.contains("IntLiteral(Decimal) \"1\""), "stdout: {out}");
    }

    #[test]
    fn dash_i_lets_system_include_resolve() {
        let dir = tmp_dir("dash-i");
        std::fs::write(dir.join("h.h"), "int y;\n").unwrap();
        let path = dir.join("a.c");
        std::fs::write(&path, "#include <h.h>\n").unwrap();
        let path_str = path.to_string_lossy().to_string();
        let inc = dir.to_string_lossy().to_string();
        let (code, out, err) = run_args(&["-I", &inc, "-emit=pp-tokens", &path_str]);
        assert_eq!(code, 0, "stderr: {err}");
        assert!(out.contains("Ident"), "stdout: {out}");
    }

    #[test]
    fn bad_define_name_errors() {
        let (code, _, err) = run_args(&["-D=42", "a.c"]);
        assert_eq!(code, 2);
        assert!(err.contains("macro name"), "stderr: {err}");
    }

    fn tmp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let mut p = std::env::temp_dir();
        let id = N.fetch_add(1, Ordering::Relaxed);
        p.push(format!("rccx-cli-{}-{}-{}", tag, std::process::id(), id));
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}
