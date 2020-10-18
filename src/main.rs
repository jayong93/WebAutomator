use anyhow::{anyhow, bail, Result};
use fantoccini::Client;
use fantoccini::{Element, Locator};
use std::fs::OpenOptions;
use std::path::PathBuf;
use structopt::StructOpt;
use tokio;
use yaml2commands::{serde_yaml::from_str, CommandType, WebCommand};

#[derive(Debug, StructOpt)]
struct CmdOption {
    #[structopt(help = "A file path which contains webdriver commands written in YAML format")]
    input_file: PathBuf,
    #[structopt(
        long,
        default_value = "geckodriver",
        help = "A path specifying where the geckodriver binary is"
    )]
    geckodriver_path: String,
}

fn main() -> Result<()> {
    use std::io::Read;
    use std::process::Command;
    let option = CmdOption::from_args();
    let mut runtime = tokio::runtime::Builder::new()
        .basic_scheduler()
        .enable_all()
        .build()?;

    let mut input_file = OpenOptions::new().read(true).open(option.input_file)?;
    let mut file_contents = String::new();
    input_file.read_to_string(&mut file_contents)?;
    let commands: Vec<WebCommand> = from_str(&file_contents)?;

    let mut child = Command::new(&option.geckodriver_path).spawn()?;
    let result = if let Ok(mut client) = runtime.block_on(Client::new("http://localhost:4444")) {
        commands
            .iter()
            .try_for_each(|command| run_command(&mut client, command, &mut runtime))
    } else {
        Ok(())
    };
    child.kill()?;

    result
}

fn run_command(
    client: &mut Client,
    command: &WebCommand,
    runtime: &mut tokio::runtime::Runtime,
) -> Result<()> {
    use std::iter::from_fn;
    let mut c = Some(command);
    // Make Recursive commands to iterator
    let mut it = from_fn(move || {
        if let Some(com) = c {
            match com.command_type {
                CommandType::Recursive(ref new_c) => {
                    c = Some(new_c.as_ref());
                }
                _ => {
                    c = None;
                }
            }
            return Some(com);
        } else {
            return None;
        }
    });

    it.try_fold(
        None,
        |elem: Option<Element>, command: &WebCommand| -> Result<Option<Element>> {
            do_command_detail(elem, command, client, runtime)
        },
    )?;
    Ok(())
}

fn do_command_detail(
    elem: Option<Element>,
    command: &WebCommand,
    client: &mut Client,
    runtime: &mut tokio::runtime::Runtime,
) -> Result<Option<Element>> {
    use fantoccini::error::CmdError;
    use webdriver::error::ErrorStatus;

    let get_selector = || -> Result<&String> {
        command
            .selector
            .as_ref()
            .ok_or_else(|| anyhow!("A command needs a selector string"))
    };
    let get_next_locator = || -> Result<Locator> { Ok(Locator::Css(get_selector()?)) };

    match &command.command_type {
        // Command types which don't need a element
        CommandType::GoTo(url) => {
            runtime.block_on(client.goto(url))?;
            Ok(elem)
        }
        CommandType::Loop(commands) => {
            loop {
                let result = commands
                    .into_iter()
                    .try_for_each(|command| run_command(client, command, runtime));
                match result {
                    Ok(_) => break,
                    Err(e) => {
                        if let Some(CmdError::Standard(e)) = e.downcast_ref::<CmdError>() {
                            if let ErrorStatus::NoSuchWindow | ErrorStatus::InvalidSessionId = e.error {
                                bail!("A loop has failed: {}", e)
                            }
                            eprintln!("Failed to finish a loop: {}; will retry...", e);
                        } else {
                            eprintln!("Failed to finish a loop: {}; will retry...", e);
                        }
                    }
                }
            }
            Ok(elem)
        }
        CommandType::ChangeWindowSize { width, height } => {
            runtime.block_on(client.set_window_size(*width, *height))?;
            Ok(elem)
        }
        CommandType::ScrollIntoView => {
            let script = format!(
                r#"document.querySelector("{}").scrollIntoView();"#,
                get_selector()?
            );
            runtime.block_on(client.execute(&script, vec![]))?;
            Ok(elem)
        }
        CommandType::WaitForSeconds(sec) => {
            std::thread::sleep(std::time::Duration::from_secs_f64(*sec));
            Ok(elem)
        }
        CommandType::ChangeWindow(i) => {
            let windows = runtime.block_on(client.windows())?;
            if let Some(window) = windows.get(*i).cloned() {
                runtime.block_on(client.switch_to_window(window))?;
                Ok(elem)
            } else {
                bail!("Couldn't find the window")
            }
        }
        CommandType::LeaveFrame => {
            *client = runtime.block_on(client.clone().enter_parent_frame())?;
            Ok(elem)
        }
        CommandType::PrintSource => {
            eprintln!("{}", runtime.block_on(client.source())?);
            Ok(elem)
        }
        CommandType::Wait => {
            let locator = get_next_locator()?;
            runtime.block_on(client.wait_for_find(locator))?;
            Ok(elem)
        }
        // Handle command types which need a element.
        _ => {
            let locator = get_next_locator()?;
            let mut new_elem: Element = if let Some(mut e) = elem {
                runtime.block_on(e.find(locator))?
            } else {
                runtime.block_on(client.find(locator))?
            };

            match &command.command_type {
                CommandType::Clear => {
                    runtime.block_on(new_elem.clear())?;
                    Ok(None)
                }
                CommandType::EnterFrame => {
                    *client = runtime.block_on(new_elem.enter_frame())?;
                    Ok(None)
                }
                CommandType::ClickUntilNavigation => {
                    let curr_url = runtime.block_on(client.current_url())?;
                    loop {
                        *client = runtime.block_on(new_elem.click())?;
                        let new_url = runtime.block_on(client.current_url())?;
                        if curr_url != new_url {
                            break Ok(None)
                        } else {
                            new_elem = runtime.block_on(client.find(get_next_locator()?))?;
                        }
                    }
                }
                CommandType::ClickUntilDomChanged => {
                    let curr_url = runtime.block_on(client.source())?;
                    loop {
                        *client = runtime.block_on(new_elem.click())?;
                        let new_url = runtime.block_on(client.source())?;
                        if curr_url != new_url {
                            break Ok(None)
                        } else {
                            new_elem = runtime.block_on(client.find(get_next_locator()?))?;
                        }
                    }
                }
                CommandType::Click => {
                    *client = runtime.block_on(new_elem.click())?;
                    Ok(None)
                }
                CommandType::Input(s) => {
                    runtime.block_on(new_elem.send_keys(s))?;
                    Ok(None)
                }
                CommandType::Recursive(_) => Ok(Some(new_elem)),
                CommandType::Check => Ok(None),
                _ => unreachable!(),
            }
        }
    }
}
