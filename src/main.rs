use anyhow::{anyhow, bail, Result};
use fantoccini::Client;
use std::fs::OpenOptions;
use std::path::PathBuf;
use structopt::StructOpt;
use tokio;
use yaml2commands::{serde_yaml::from_str, CommandType, WebCommand};

#[derive(Debug, StructOpt)]
struct CmdOption {
    input_file: PathBuf,
    #[structopt(default_value = "geckodriver")]
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
    let result = if let Ok(client) = runtime.block_on(Client::new("http://localhost:4444")) {
        run_commands(client, commands, runtime)
    } else {
        Ok(())
    };
    child.kill()?;

    result
}

fn run_commands(
    mut client: Client,
    commands: Vec<WebCommand>,
    mut runtime: tokio::runtime::Runtime,
) -> Result<()> {
    use fantoccini::{Element, Locator};
    use std::iter::from_fn;
    for command in commands {
        let mut c = Some(&command);
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
            |mut elem: Option<Element>, command: &WebCommand| -> Result<Option<Element>> {
                let get_next_locator = || -> Result<Locator> {
                    Ok(Locator::Css(command.selector.as_ref().ok_or_else(
                        || anyhow!("A command needs a selector string"),
                    )?))
                };

                match &command.command_type {
                    CommandType::GoTo(url) => {
                        runtime.block_on(client.goto(url))?;
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
                    CommandType::EnterFrame(idx) => {
                        client = runtime.block_on(client.clone().enter_frame(Some(*idx as _)))?;
                        Ok(elem)
                    }
                    CommandType::LeaveFrame => {
                        client = runtime.block_on(client.clone().enter_parent_frame())?;
                        Ok(elem)
                    }
                    CommandType::PrintSource => {
                        eprintln!("{}", runtime.block_on(client.source())?);
                        Ok(elem)
                    }
                    CommandType::Wait => {
                        let locator = get_next_locator()?;
                        elem = Some(runtime.block_on(client.wait_for_find(locator))?);
                        Ok(elem)
                    }
                    _ => {
                        let locator = get_next_locator()?;
                        let mut new_elem: Element = if let Some(mut e) = elem {
                            runtime.block_on(e.find(locator))?
                        } else {
                            runtime.block_on(client.find(locator))?
                        };

                        elem = Some(new_elem.clone());

                        match &command.command_type {
                            CommandType::Click => {
                                client = runtime.block_on(new_elem.click())?;
                            }
                            CommandType::Input(s) => {
                                runtime.block_on(new_elem.send_keys(s))?;
                            }
                            CommandType::Recursive(_) => {}
                            _ => unreachable!(),
                        }

                        Ok(elem)
                    }
                }
            },
        )?;
    }
    Ok(())
}