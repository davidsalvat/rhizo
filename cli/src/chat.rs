//! `rhizo chat`: talk to an agent in a cell (design D5 of
//! `add-interactive-chat`). Each line typed goes to the cell's standard
//! input; the agent (`cells/python/agent_chat.py`) answers on its output and
//! ends each answer with a line giving its times, which this splits into
//! the model's time and Rhizo's. Commands fork the conversation, switch
//! between its branches, suspend or end it. The kernel only executes: which
//! branch receives input is decided here.

use crate::client::{Client, duration};
use crate::{agent, args, describe};
use rhizo_control::{Body, MAX_INPUT, Message, Request, Run, State};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, IsTerminal, Write};
use std::time::Instant;
use xtask::control::{CHAT_READY, ChatTurn};
use xtask::vmm::Vmm;

pub const HELP: &str = "\
Type a message and press Enter: the agent answers, calling its tools (Python
snippets, files) inside the cell. Commands:
  /fork K      Fork the conversation into K new branches (copy-on-write);
               you stay on this one
  /branches    List the branches, their cells and states
  /switch N    Talk to branch (cell) N, resuming it if suspended
  /suspend     Suspend this branch and leave (`rhizo chat N` resumes it)
  /exit        End this branch and leave
  /help        This text
Ctrl-D closes the agent's input: it ends, and so does the chat.";

/// Most branches one `/fork` makes.
const MAX_FORK: u32 = 64;

/// What a line typed in the chat asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// A message for the agent.
    Say(String),
    Fork(u32),
    Branches,
    Switch(u32),
    Suspend,
    Exit,
    Help,
    /// An empty line.
    Nothing,
}

/// Reads one typed line.
pub fn parse_line(line: &str) -> Result<Line, String> {
    let line = line.trim();
    let Some(command) = line.strip_prefix('/') else {
        if line.len() >= MAX_INPUT {
            return Err(format!("a message holds at most {} bytes", MAX_INPUT - 1));
        }
        return Ok(if line.is_empty() {
            Line::Nothing
        } else {
            Line::Say(line.to_string())
        });
    };
    let words: Vec<&str> = command.split_whitespace().collect();
    let number = |what: &str, word: &str| {
        word.parse::<u32>()
            .map_err(|_| format!("{what}: `{word}` is not a number"))
    };
    Ok(match words.as_slice() {
        ["fork", k] => match number("/fork", k)? {
            k @ 1..=MAX_FORK => Line::Fork(k),
            _ => return Err(format!("/fork makes 1 to {MAX_FORK} branches")),
        },
        ["fork"] => Line::Fork(1),
        ["branches"] => Line::Branches,
        ["switch", n] => Line::Switch(number("/switch", n)?),
        ["suspend"] => Line::Suspend,
        ["exit" | "quit"] => Line::Exit,
        ["help"] => Line::Help,
        _ => return Err(format!("unknown command `{line}`: /help lists them")),
    })
}

fn say(text: &str) {
    println!("[rhizo] {text}");
}

/// The time split of one turn: the model's, then Rhizo's (everything
/// else), which is the agent's own work in the cell plus the delivery of
/// the message and the answer (kernel, serial line, host).
pub fn turn_line(total_ns: u64, turn: ChatTurn) -> String {
    let rhizo = total_ns.saturating_sub(turn.model_ns);
    let cell = turn.agent_ns.saturating_sub(turn.model_ns);
    format!(
        "model {}, rhizo {} (agent in the cell {}, delivery {})",
        duration(turn.model_ns),
        duration(rhizo),
        duration(cell),
        duration(rhizo.saturating_sub(cell))
    )
}

struct Session<'a> {
    client: &'a mut Client,
    /// The branch receiving input.
    current: u32,
    /// The branches known to this session, in the order met.
    branches: Vec<u32>,
    /// Cells whose output this session streams.
    watched: HashSet<u32>,
    /// Each cell's output after its last full line.
    partial: HashMap<u32, Vec<u8>>,
}

/// `rhizo chat`: starts or resumes the conversation, then talks.
pub fn run(client: &mut Client, chat: &args::Chat) -> Result<(), String> {
    let mut session = match chat {
        args::Chat::New { llm, model } => start(client, llm, model)?,
        args::Chat::Cell(id) => {
            let mut session = Session::new(client, *id);
            let resumed = session.wake(*id)?;
            session.watch(*id)?;
            say(&format!(
                "talking to cell {id}{resumed}; /help for commands"
            ));
            session
        }
    };
    session.talk()
}

fn start<'a>(client: &'a mut Client, llm: &str, model: &str) -> Result<Session<'a>, String> {
    let t0 = Instant::now();
    let url = format!(
        "http://{}",
        agent::guest_endpoint(llm, Vmm::parse(&client.info.vmm)?)?
    );
    let reply = client.call(&Request::Run(Run {
        image: b"cpython".to_vec(),
        files_from: b"cpython-files".to_vec(),
        argv: ["/usr/bin/python3", "/app/agent_chat.py", &url, model]
            .iter()
            .map(|a| a.as_bytes().to_vec())
            .collect(),
        input: true,
        ..Run::default()
    }))?;
    let &Body::Cell(id) = &reply.body else {
        return Err(format!("unexpected reply {:?}", reply.body));
    };
    say(&format!(
        "chat cell {id}: CPython with `agent_chat.py`, model `{model}` at {url} as the guest \
         sees it; started in {}",
        reply.timing()
    ));
    let mut session = Session::new(client, id);
    session.watch(id)?;
    if session.wait(|l| (l == CHAT_READY).then_some(()))?.is_none() {
        return Err(format!("the agent in cell {id} ended before it was ready"));
    }
    say(&format!(
        "ready {} after the request; type a message, /help for commands",
        duration(t0.elapsed().as_nanos() as u64)
    ));
    Ok(session)
}

impl<'a> Session<'a> {
    fn new(client: &'a mut Client, id: u32) -> Self {
        Session {
            client,
            current: id,
            branches: vec![id],
            watched: HashSet::new(),
            partial: HashMap::new(),
        }
    }

    /// Streams cell `id`'s output, once.
    fn watch(&mut self, id: u32) -> Result<(), String> {
        if self.watched.insert(id) {
            self.client.send(&Request::Output(id))?;
        }
        Ok(())
    }

    /// The state of cell `id`, as the kernel lists it.
    fn state(&mut self, id: u32) -> Result<State, String> {
        let reply = self.client.call(&Request::List)?;
        let Body::List(cells) = reply.body else {
            return Err(format!("unexpected reply {:?}", reply.body));
        };
        cells
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.state)
            .ok_or_else(|| format!("no cell {id}"))
    }

    /// Makes cell `id` run: resumes it if suspended; returns what to say
    /// about that.
    fn wake(&mut self, id: u32) -> Result<String, String> {
        match self.state(id)? {
            State::Running => Ok(String::new()),
            State::Suspended { .. } => {
                let reply = self.client.call(&Request::Resume(id))?;
                Ok(format!(", resumed in {}", reply.timing()))
            }
            ended => Err(format!("cell {id} {}: nothing to talk to", describe(ended))),
        }
    }

    /// Shows the current branch's output until `done` takes one of its
    /// lines; `None` if the branch ended first. Lines of other branches
    /// are shown with their cell; the agent's marker lines are not shown.
    fn wait<T>(&mut self, mut done: impl FnMut(&str) -> Option<T>) -> Result<Option<T>, String> {
        let mut out = std::io::stdout();
        loop {
            match self.client.next()?.1 {
                Message::Output { cell, data } => {
                    let buf = self.partial.entry(cell).or_default();
                    buf.extend_from_slice(&data);
                    let mut lines = Vec::new();
                    while let Some(end) = buf.iter().position(|&b| b == b'\n') {
                        let line: Vec<u8> = buf.drain(..=end).collect();
                        lines.push(String::from_utf8_lossy(&line[..end]).into_owned());
                    }
                    for line in lines {
                        if cell == self.current
                            && let Some(found) = done(&line)
                        {
                            return Ok(Some(found));
                        }
                        if line.starts_with('\x1e') {
                            continue;
                        }
                        let _ = match cell == self.current {
                            true => writeln!(out, "{line}"),
                            false => writeln!(out, "[cell {cell}] {line}"),
                        };
                    }
                    let _ = out.flush();
                }
                Message::Ended { cell, state } => {
                    self.watched.remove(&cell);
                    say(&format!("cell {cell} {}", describe(state)));
                    if cell == self.current {
                        return Ok(None);
                    }
                }
                Message::Failed { error, message, .. } => {
                    return Err(format!(
                        "{}: {}",
                        error.as_str(),
                        String::from_utf8_lossy(&message)
                    ));
                }
                _ => {}
            }
        }
    }

    /// Reads lines until the conversation is left.
    fn talk(&mut self) -> Result<(), String> {
        let stdin = std::io::stdin();
        let typed = stdin.is_terminal();
        let mut input = stdin.lock();
        loop {
            if typed {
                print!("[{}]> ", self.current);
                let _ = std::io::stdout().flush();
            }
            let mut line = String::new();
            if input.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
                // Ctrl-D: the agent reads the end of its input and ends.
                if typed {
                    println!();
                }
                self.client.call(&Request::CloseInput(self.current))?;
                self.wait(|_| None::<()>)?;
                return Ok(());
            }
            if !typed {
                println!("> {}", line.trim_end());
            }
            let step = match parse_line(&line) {
                Ok(Line::Nothing) => Ok(true),
                Ok(Line::Help) => {
                    println!("{HELP}");
                    Ok(true)
                }
                Ok(Line::Say(text)) => self.turn(&text),
                Ok(Line::Fork(k)) => self.fork(k),
                Ok(Line::Branches) => self.list(),
                Ok(Line::Switch(id)) => self.switch(id),
                Ok(Line::Suspend) => {
                    let reply = self.client.call(&Request::Suspend(self.current))?;
                    let kib = match reply.body {
                        Body::Bytes(b) => b / 1024,
                        _ => 0,
                    };
                    say(&format!(
                        "suspended cell {} ({kib} KiB snapshot) in {}; `rhizo chat {}` resumes it",
                        self.current,
                        reply.timing(),
                        self.current
                    ));
                    return Ok(());
                }
                Ok(Line::Exit) => {
                    let reply = self.client.call(&Request::Kill(self.current))?;
                    say(&format!(
                        "ended cell {} in {}",
                        self.current,
                        reply.timing()
                    ));
                    let others: Vec<u32> = self
                        .branches
                        .iter()
                        .copied()
                        .filter(|&b| b != self.current)
                        .collect();
                    if !others.is_empty() {
                        say(&format!(
                            "other branches may still run: {others:?} (`rhizo chat N`, `rhizo kill N`)"
                        ));
                    }
                    return Ok(());
                }
                Err(e) => Err(e),
            };
            match step {
                Ok(true) => {}
                Ok(false) => return Ok(()),
                Err(e) => say(&format!("error: {e}")),
            }
        }
    }

    /// One message and its answer; `false` if the branch ended.
    fn turn(&mut self, text: &str) -> Result<bool, String> {
        let t0 = Instant::now();
        self.client.call(&Request::Input {
            cell: self.current,
            data: format!("{text}\n").into_bytes(),
        })?;
        match self.wait(ChatTurn::parse)? {
            Some(turn) => {
                say(&turn_line(t0.elapsed().as_nanos() as u64, turn));
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn fork(&mut self, k: u32) -> Result<bool, String> {
        let reply = self.client.call(&Request::Fork {
            cell: self.current,
            n: k,
        })?;
        let Body::Cells(children) = &reply.body else {
            return Err(format!("unexpected reply {:?}", reply.body));
        };
        self.branches.extend(children);
        say(&format!(
            "forked cell {} into {children:?} (copy-on-write, the conversation shared) in {}; \
             still on {}",
            self.current,
            reply.timing(),
            self.current
        ));
        Ok(true)
    }

    fn list(&mut self) -> Result<bool, String> {
        let reply = self.client.call(&Request::List)?;
        let Body::List(cells) = &reply.body else {
            return Err(format!("unexpected reply {:?}", reply.body));
        };
        for &b in &self.branches {
            let state = cells
                .iter()
                .find(|c| c.id == b)
                .map_or("forgotten".to_string(), |c| describe(c.state));
            let mark = if b == self.current { '*' } else { ' ' };
            say(&format!("{mark} {b:<6} {state}"));
        }
        say(&format!(
            "{} branches, listed in {}",
            self.branches.len(),
            reply.timing()
        ));
        Ok(true)
    }

    fn switch(&mut self, id: u32) -> Result<bool, String> {
        let resumed = self.wake(id)?;
        if !self.branches.contains(&id) {
            self.branches.push(id);
        }
        self.watch(id)?;
        self.current = id;
        say(&format!("now talking to cell {id}{resumed}"));
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_messages_or_commands() {
        assert_eq!(
            parse_line("hello there\n"),
            Ok(Line::Say("hello there".into()))
        );
        assert_eq!(parse_line("  \n"), Ok(Line::Nothing));
        assert_eq!(parse_line("/fork 3"), Ok(Line::Fork(3)));
        assert_eq!(parse_line("/fork"), Ok(Line::Fork(1)));
        assert_eq!(parse_line("/branches"), Ok(Line::Branches));
        assert_eq!(parse_line("/switch 12\n"), Ok(Line::Switch(12)));
        assert_eq!(parse_line("/suspend"), Ok(Line::Suspend));
        assert_eq!(parse_line("/exit"), Ok(Line::Exit));
        assert_eq!(parse_line("/quit"), Ok(Line::Exit));
        assert_eq!(parse_line("/help"), Ok(Line::Help));
        assert!(parse_line("/fork 0").is_err());
        assert!(parse_line("/fork 65").is_err());
        assert!(parse_line("/fork many").is_err());
        assert!(parse_line("/switch").is_err());
        assert!(parse_line("/switch x").is_err());
        assert!(parse_line("/teleport").is_err());
        assert!(parse_line(&"a".repeat(MAX_INPUT)).is_err());
        assert!(parse_line(&"a".repeat(MAX_INPUT - 1)).is_ok());
    }

    #[test]
    fn a_turn_splits_model_and_rhizo_time() {
        let turn = ChatTurn {
            model_ns: 2_000_000_000,
            agent_ns: 2_030_000_000,
        };
        assert_eq!(
            turn_line(2_045_000_000, turn),
            "model 2000.0 ms, rhizo 45.0 ms (agent in the cell 30.0 ms, delivery 15.0 ms)"
        );
        // Clocks of the cell and the host never make a negative time.
        assert!(turn_line(1, turn).contains("rhizo 0 us"));
    }
}
