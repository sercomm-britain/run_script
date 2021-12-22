//! # command
//!
//! Runs task commands/scripts.
//!

#[cfg(test)]
#[path = "./runner_test.rs"]
mod runner_test;

use crate::types::{IoOptions, ScriptError, ScriptOptions, ScriptResult};
use fsio;
use fsio::path::from_path::FromPath;
use fsio::types::FsIOResult;
use std::env::current_dir;
use std::process::{Child, Command, ExitStatus, Stdio};

#[cfg(test)]
fn exit(code: i32) -> ! {
    panic!("{}", code);
}

#[cfg(not(test))]
use std::process::exit;

/// Returns the exit code
fn get_exit_code(code: ExitStatus) -> i32 {
    if !code.success() {
        match code.code() {
            Some(value) => value,
            None => -1,
        }
    } else {
        0
    }
}

/// Creates a command builder for the given input.
fn create_command_builder(
    command_string: &str,
    args: &Vec<String>,
    options: &ScriptOptions,
) -> Command {
    let mut command = Command::new(&command_string);

    if options.env_vars.is_some() {
        command.envs(options.env_vars.as_ref().unwrap());
    }

    for arg in args.iter() {
        command.arg(arg);
    }

    match options.input_redirection {
        IoOptions::Null => command.stdin(Stdio::null()),
        IoOptions::Inherit => command.stdin(Stdio::inherit()),
        IoOptions::Pipe => command.stdin(Stdio::piped()),
    };

    match options.output_redirection {
        IoOptions::Null => command.stdout(Stdio::null()).stderr(Stdio::null()),
        IoOptions::Inherit => command.stdout(Stdio::inherit()).stderr(Stdio::inherit()),
        IoOptions::Pipe => command.stdout(Stdio::piped()).stderr(Stdio::piped()),
    };

    command
}

fn create_script_file(script: &String) -> FsIOResult<String> {
    let extension = if cfg!(windows) { "bat" } else { "sh" };
    let file_path = fsio::path::get_temporary_file_path(extension);

    match fsio::file::write_text_file(&file_path, script) {
        Ok(_) => Ok(file_path),
        Err(error) => {
            fsio::file::delete_ignore_error(&file_path);

            Err(error)
        }
    }
}

fn fix_path(path_string: &str) -> String {
    if cfg!(windows) {
        fsio::path::canonicalize_or(&path_string, &path_string)
    } else {
        path_string.to_string()
    }
}

fn modify_script(script: &String, options: &ScriptOptions) -> ScriptResult<String> {
    match current_dir() {
        Ok(cwd_holder) => {
            match cwd_holder.to_str() {
                Some(cwd) => {
                    let cwd_string = fix_path(cwd);

                    // create cd command
                    let mut cd_command = "cd \"".to_string();
                    cd_command.push_str(&cwd_string);
                    cd_command.push('"');
                    if let Some(ref working_directory) = options.working_directory {
                        cd_command.push_str(" && cd \"");
                        let working_directory_string: String =
                            FromPath::from_path(&working_directory);
                        cd_command.push_str(&working_directory_string);
                        cd_command.push('"');
                    }

                    let mut script_lines: Vec<String> = script
                        .trim()
                        .split("\n")
                        .map(|string| string.to_string())
                        .collect();

                    // check if first line is shebang line
                    let mut insert_index =
                        if script_lines.len() > 0 && script_lines[0].starts_with("#!") {
                            1
                        } else {
                            0
                        };

                    if !cfg!(windows) {
                        if options.exit_on_error {
                            script_lines.insert(insert_index, "set -e".to_string());
                            insert_index = insert_index + 1;
                        }

                        if options.print_commands {
                            script_lines.insert(insert_index, "set -x".to_string());
                            insert_index = insert_index + 1;
                        }
                    }

                    script_lines.insert(insert_index, cd_command);

                    script_lines.push("\n".to_string());

                    let updated_script = script_lines.join("\n");

                    Ok(updated_script)
                }
                None => Err(ScriptError::Description(
                    "Unable to extract current working directory path.",
                )),
            }
        }
        Err(error) => Err(ScriptError::IOError(error)),
    }
}

/// Invokes the provided script content and returns a process handle.
fn spawn_script(
    script: &str,
    args: &Vec<String>,
    options: &ScriptOptions,
) -> ScriptResult<(Child, String)> {
    match modify_script(&script.to_string(), &options) {
        Ok(updated_script) => match create_script_file(&updated_script) {
            Ok(file) => {
                let command = match options.runner {
                    Some(ref value) => value,
                    None => {
                        if cfg!(windows) {
                            "cmd.exe"
                        } else {
                            "sh"
                        }
                    }
                };

                let mut all_args = if cfg!(windows) {
                    let win_file = fix_path(&file);
                    vec!["/C".to_string(), win_file]
                } else {
                    vec![file.to_string()]
                };

                all_args.extend(args.iter().cloned());

                let mut command = create_command_builder(&command, &all_args, &options);

                let result = command.spawn();

                match result {
                    Ok(child) => Ok((child, file.clone())),
                    Err(error) => {
                        fsio::file::delete_ignore_error(&file);

                        Err(ScriptError::IOError(error))
                    }
                }
            }
            Err(error) => Err(ScriptError::FsIOError(error)),
        },
        Err(error) => Err(error),
    }
}

/// Invokes the provided script content and returns a process handle.
///
/// # Arguments
///
/// * `script` - The script content
/// * `args` - The script command line arguments
/// * `options` - Options provided to the script runner
pub(crate) fn spawn(
    script: &str,
    args: &Vec<String>,
    options: &ScriptOptions,
) -> ScriptResult<Child> {
    let result = spawn_script(script, &args, &options);

    match result {
        Ok((child, _)) => Ok(child),
        Err(error) => Err(error),
    }
}

/// Invokes the provided script content and returns the invocation output.
///
/// # Arguments
///
/// * `script` - The script content
/// * `args` - The script command line arguments
/// * `options` - Options provided to the script runner
pub(crate) fn run(
    script: &str,
    args: &Vec<String>,
    options: &ScriptOptions,
) -> ScriptResult<(i32, String, String)> {
    let result = spawn_script(script, &args, &options);

    match result {
        Ok((child, file)) => {
            let process_result = child.wait_with_output();

            fsio::file::delete_ignore_error(&file);

            match process_result {
                Ok(output) => {
                    let exit_code = get_exit_code(output.status);
                    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
                    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

                    Ok((exit_code, stdout, stderr))
                }
                Err(error) => Err(ScriptError::IOError(error)),
            }
        }
        Err(error) => Err(error),
    }
}

/// Invokes the provided script content and returns the invocation output.
/// In case of invocation error or error exit code, this function will exit the main process.
///
/// # Arguments
///
/// * `script` - The script content
/// * `args` - The script command line arguments
/// * `options` - Options provided to the script runner
pub(crate) fn run_or_exit(
    script: &str,
    args: &Vec<String>,
    options: &ScriptOptions,
) -> (String, String) {
    let result = run(script, &args, &options);

    match result {
        Ok((exit_code, output, error)) => {
            if exit_code != 0 {
                eprintln!("{}", error);
                exit(exit_code)
            } else {
                (output, error)
            }
        }
        Err(error) => {
            eprintln!("{}", error);
            exit(1)
        }
    }
}

#[cfg(feature = "async")]
fn create_command_builder_async(
    command_string: &str,
    args: &Vec<String>,
    options: &ScriptOptions,
) -> async_process::Command {
    let mut command = async_process::Command::new(&command_string);

    if options.env_vars.is_some() {
        command.envs(options.env_vars.as_ref().unwrap());
    }

    for arg in args.iter() {
        command.arg(arg);
    }

    match options.input_redirection {
        IoOptions::Null => command.stdin(async_process::Stdio::null()),
        IoOptions::Inherit => command.stdin(async_process::Stdio::inherit()),
        IoOptions::Pipe => command.stdin(async_process::Stdio::piped()),
    };

    match options.output_redirection {
        IoOptions::Null => command
            .stdout(async_process::Stdio::null())
            .stderr(async_process::Stdio::null()),
        IoOptions::Inherit => command
            .stdout(async_process::Stdio::inherit())
            .stderr(async_process::Stdio::inherit()),
        IoOptions::Pipe => command
            .stdout(async_process::Stdio::piped())
            .stderr(async_process::Stdio::piped()),
    };

    command
}

#[cfg(feature = "async")]
fn spawn_script_async(
    script: &str,
    args: &Vec<String>,
    options: &ScriptOptions,
) -> Result<(async_process::Child, String), ScriptError> {
    match modify_script(&script.to_string(), &options) {
        Ok(updated_script) => match create_script_file(&updated_script) {
            Ok(file) => {
                let command = match options.runner {
                    Some(ref value) => value,
                    None => {
                        if cfg!(windows) {
                            "cmd.exe"
                        } else {
                            "sh"
                        }
                    }
                };

                let mut all_args = if cfg!(windows) {
                    let win_file = fix_path(&file);
                    vec!["/C".to_string(), win_file]
                } else {
                    vec![file.to_string()]
                };

                all_args.extend(args.iter().cloned());

                let mut command = create_command_builder_async(&command, &all_args, &options);

                let result = command.spawn();

                match result {
                    Ok(child) => Ok((child, file.clone())),
                    Err(error) => {
                        fsio::file::delete_ignore_error(&file);

                        Err(ScriptError::IOError(error))
                    }
                }
            }
            Err(error) => Err(ScriptError::FsIOError(error)),
        },
        Err(error) => Err(error),
    }
}

#[cfg(feature = "async")]
pub fn spawn_async(
    script: &str,
    args: &Vec<String>,
    options: &ScriptOptions,
) -> Result<async_process::Child, ScriptError> {
    let result = spawn_script_async(script, &args, &options);

    match result {
        Ok((child, _)) => Ok(child),
        Err(error) => Err(error),
    }
}
