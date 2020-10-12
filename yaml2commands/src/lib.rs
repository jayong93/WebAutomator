use serde::{Serialize, Deserialize};
pub use serde_yaml;

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub enum CommandType {
    Click,
    Check,
    Input(String),
    Wait,
    WaitForSeconds(f64),
    GoTo(String),
    ChangeWindow(usize),
    EnterFrame,
    LeaveFrame,
    PrintSource,
    Recursive(Box<WebCommand>),
    ScrollIntoView,
    ChangeWindowSize{width: u32, height: u32},
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct WebCommand {
    pub selector: Option<String>,
    pub command_type: CommandType,
}

#[cfg(test)]
mod tests {
    use crate::*;
    use serde_yaml::{to_string, from_str};
    #[test]
    fn serialize_command_types() {
        let expected_str =
"---
- Click
- Input: test
- Wait";
        let command_types = vec![CommandType::Click, CommandType::Input("test".into()), CommandType::Wait];
        assert_eq!(to_string(&command_types).unwrap(), expected_str);
    }

    #[test]
    fn serialize_whole_command() {
        let commands = vec![
            WebCommand{
                selector: None,
                command_type: CommandType::GoTo("https://google.com".into())
            },
            WebCommand{
                selector: None,
                command_type: CommandType::ChangeWindowSize{width: 800, height: 600},
            },
            WebCommand{
                selector: Some("a#link".into()),
                command_type: CommandType::Click,
            },
            WebCommand{
                selector: Some("div".into()),
                command_type: CommandType::Recursive(Box::new(
                    WebCommand {
                        selector: Some("input".into()),
                        command_type: CommandType::Input("input text".into())
                    }
                ))
            }
        ];
        let expected_str =
"---
- selector: ~
  command_type:
    GoTo: \"https://google.com\"
- selector: ~
  command_type:
    ChangeWindowSize:
      width: 800
      height: 600
- selector: \"a#link\"
  command_type: Click
- selector: div
  command_type:
    Recursive:
      selector: input
      command_type:
        Input: input text";
        assert_eq!(to_string(&commands).unwrap(), expected_str);
    }

    #[test]
    fn deserialize_test() {
        let input_str =
"---
- selector: p.test
  command_type: Wait
- selector: \"a#link\"
  command_type: Click
- selector: div
  command_type:
    Recursive:
      selector: input
      command_type:
        Input: input text";
        let commands = vec![
            WebCommand{
                selector: Some("p.test".into()),
                command_type: CommandType::Wait,
            },
            WebCommand{
                selector: Some("a#link".into()),
                command_type: CommandType::Click,
            },
            WebCommand{
                selector: Some("div".into()),
                command_type: CommandType::Recursive(Box::new(
                    WebCommand {
                        selector: Some("input".into()),
                        command_type: CommandType::Input("input text".into())
                    }
                ))
            }
        ];
        assert_eq!(from_str::<Vec<WebCommand>>(input_str).unwrap(), commands);
    }
}
