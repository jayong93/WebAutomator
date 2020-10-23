use anyhow::{anyhow, bail, Result};
use fantoccini::Client;
use fantoccini::{Element, Locator};
use futures::prelude::*;
use std::fs::OpenOptions;
use std::path::PathBuf;
use std::time::Duration;
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

#[tokio::main]
async fn main() -> Result<()> {
    use std::io::Read;
    use std::process::Command;
    let option = CmdOption::from_args();

    let mut input_file = OpenOptions::new().read(true).open(option.input_file)?;
    let mut file_contents = String::new();
    input_file.read_to_string(&mut file_contents)?;
    let commands: Vec<WebCommand> = from_str(&file_contents)?;

    let mut child = Command::new(&option.geckodriver_path).spawn()?;
    let mut client = Client::new("http://localhost:4444").await?;
    for command in commands {
        if let Err(e) = run_command(&mut client, &command).await {
            eprintln!("Error has occured: {}", e);
            break;
        }
    }
    child.kill()?;

    Ok(())
}

fn run_command<'c>(
    client: &'c mut Client,
    command: &'c WebCommand,
) -> future::BoxFuture<'c, Result<()>> {
    use std::iter::from_fn;
    let mut c = Some(command);
    // Make Recursive commands to iterator
    let it = from_fn(move || {
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

    async move {
        stream::iter(it)
            .map(Result::Ok)
            .try_fold((None, client), |(elem, client), command| async move {
                do_command_detail(elem, command, client)
                    .await
                    .map(|e| (e, client))
            })
            .await?;
        Ok(())
    }
    .boxed()
}

async fn do_command_detail(
    elem: Option<Element>,
    command: &WebCommand,
    client: &mut Client,
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
            client.goto(url).await?;
            Ok(elem)
        }
        CommandType::Loop(commands) => {
            loop {
                let mut result = Ok(());
                for command in commands {
                    result = run_command(client, command).await;
                    if result.is_err() {
                        break;
                    }
                }
                match result {
                    Ok(_) => break,
                    Err(e) => {
                        if let Some(CmdError::Standard(e)) = e.downcast_ref::<CmdError>() {
                            if let ErrorStatus::NoSuchWindow | ErrorStatus::InvalidSessionId =
                                e.error
                            {
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
            client.set_window_size(*width, *height).await?;
            Ok(elem)
        }
        CommandType::ScrollIntoView => {
            let script = format!(
                r#"document.querySelector("{}").scrollIntoView();"#,
                get_selector()?
            );
            client.execute(&script, vec![]).await?;
            Ok(elem)
        }
        CommandType::WaitForSeconds(sec) => {
            if let Ok(locator) = get_next_locator() {
                tokio::time::timeout(Duration::from_secs_f64(*sec), client.wait_for_find(locator))
                    .await??;
            } else {
                std::thread::sleep(std::time::Duration::from_secs_f64(*sec));
            }
            Ok(elem)
        }
        CommandType::ChangeWindow(i) => {
            let windows = client.windows().await?;
            if let Some(window) = windows.get(*i).cloned() {
                client.switch_to_window(window).await?;
                Ok(elem)
            } else {
                bail!("Couldn't find the window")
            }
        }
        CommandType::LeaveFrame => {
            *client = client.clone().enter_parent_frame().await?;
            Ok(elem)
        }
        CommandType::PrintSource => {
            eprintln!("{}", client.source().await?);
            Ok(elem)
        }
        CommandType::Wait => {
            let locator = get_next_locator()?;
            client.wait_for_find(locator).await?;
            Ok(elem)
        }
        // Handle command types which need a element.
        _ => {
            let locator = get_next_locator()?;
            let mut new_elem: Element = if let Some(mut e) = elem {
                e.find(locator).await?
            } else {
                client.find(locator).await?
            };

            match &command.command_type {
                CommandType::Clear => {
                    new_elem.clear().await?;
                    Ok(None)
                }
                CommandType::EnterFrame => {
                    *client = new_elem.enter_frame().await?;
                    Ok(None)
                }
                CommandType::ClickUntilNavigation => {
                    let curr_url = client.current_url().await?;
                    loop {
                        *client = new_elem.click().await?;
                        let new_url = client.current_url().await?;
                        if curr_url != new_url {
                            break Ok(None);
                        } else {
                            new_elem = client.find(get_next_locator()?).await?;
                        }
                    }
                }
                CommandType::ClickUntilDomChanged => {
                    let curr_url = client.source().await?;
                    loop {
                        *client = new_elem.click().await?;
                        let new_url = client.source().await?;
                        if curr_url != new_url {
                            break Ok(None);
                        } else {
                            new_elem = client.find(get_next_locator()?).await?;
                        }
                    }
                }
                CommandType::Click => {
                    *client = new_elem.click().await?;
                    Ok(None)
                }
                CommandType::Input(s) => {
                    new_elem.send_keys(s).await?;
                    Ok(None)
                }
                CommandType::Recursive(_) => Ok(Some(new_elem)),
                CommandType::Check => Ok(None),
                _ => unreachable!(),
            }
        }
    }
}
