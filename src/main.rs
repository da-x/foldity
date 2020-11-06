use anyhow::Result;
use thiserror::Error;
use structopt::StructOpt;
use termion::screen::AlternateScreen;
use std::io::{Write, stdout};
use slab::Slab;
use regex::{RegexSet, Regex};

mod cmdline;
mod program;
mod display;
mod util;

use util::most_equal_divide;
use program::Program;
use display::DisplayKind;
use futures::channel::mpsc;

type Sender<T> = mpsc::UnboundedSender<T>;
type Receiver<T> = mpsc::UnboundedReceiver<T>;
type Key = usize;
type Text = String;
type PairId = usize;


#[derive(Error, Debug)]
pub(crate) enum Error {
    #[error("Start and end matchers count dont match: {0} != {1}")]
    MatchPairInvalid(usize, usize),

    #[error("No captures for regex {0}")]
    ExpectedCaptures(String),

    #[error("Multiple captures and no capture named M for regex {0}")]
    CaptureNameNotFound(String),
}

struct Encapsulation {
    #[allow(unused)]
    pair_id: PairId,
    start_title: Text,
    end_title: Option<Text>,
    start_line: Text,
    end_line: Option<Text>,
    content: Vec<Output>,
}

impl Encapsulation {
    fn is_ended(&self) -> bool {
        self.end_title.is_some()
    }
}

enum Output {
    Line(Text),
    Encapsulation(Encapsulation),
}

struct MatchPair {
    start: Regex,
    end: Regex,
}

struct Matchers<'a> {
    match_pairs: &'a Vec<MatchPair>,
    regex_set: &'a RegexSet,
}

struct Main {
    receiver: Receiver<(Key, Result<Text, std::io::Error>)>,
    sender: Option<Sender<(Key, Result<Text, std::io::Error>)>>,
    opt: cmdline::Opt,
    programs: Slab<Program>,
    match_pairs: Vec<MatchPair>,
    regex_set: RegexSet,
}

enum DrawMode {
    Ongoing,
    Final,
}

impl Main {
    fn new() -> Self {
        let (broker_sender, broker_receiver) = mpsc::unbounded();

        let a : &[&String] = &[];
        Self {
            opt: cmdline::Opt::from_args(),
            programs: Slab::new(),
            receiver: broker_receiver,
            sender: Some(broker_sender),
            match_pairs: vec![],
            regex_set: RegexSet::new(a).unwrap(),
        }
    }

    fn regex(s: &str) -> Result<Regex> {
        let r = Regex::new(&format!("^{}$", s))?;

        if r.captures_len() == 1 {
            return Err(Error::ExpectedCaptures(String::from(s)).into());
        }

        if r.captures_len() > 2 {
            let mut found = false;
            for name in r.capture_names() {
                if name == Some("M") {
                    found  = true;
                }
            }

            if !found {
                return Err(Error::CaptureNameNotFound(String::from(s)).into());
            }
        }

        Ok(r)
    }

    fn run(&mut self) -> Result<()> {
        let s = self.opt.match_start.len();
        let e = self.opt.match_end.len();
        if s != e {
            return Err(Error::MatchPairInvalid(e, s).into());
        }

        let mut regex_set = vec![];
        for (start, end) in itertools::zip(&self.opt.match_start, &self.opt.match_end) {
            let start = Self::regex(start)?;
            let end = Self::regex(end)?;
            let pair = MatchPair { start, end };
            regex_set.push(String::from(pair.start.as_str()));
            regex_set.push(String::from(pair.end.as_str()));
            self.match_pairs.push(pair);
        }
        self.regex_set = RegexSet::new(&regex_set)?;

        if self.opt.programs.is_empty() {
            self.insert_stdin()?;
        } else {
            todo!("support multiple programs specified in command line");
        }

        drop(self.sender.take());

        if !self.opt.replay || self.opt.debug {
            async_std::task::block_on(async { let _ = self.run_loop().await; });
        } else {
            {
                let mut screen = AlternateScreen::from(stdout());
                async_std::task::block_on(async { let _ = self.run_loop().await; });
                screen.flush().unwrap();
            }

            self.end_execution()?;
        }

        if self.opt.debug {
            self.end_execution()?;
        }

        Ok(())
    }

    fn insert_stdin(&mut self) -> Result<()> {
        let entry = self.programs.vacant_entry();
        let key = entry.key();
        let broker_sender = self.sender.clone().unwrap();

        async_std::task::spawn(async move {
            let _res = Self::stdin_loop(key, broker_sender).await;
        });

        entry.insert(Program::new("<<stdin>>".to_owned()));

        Ok(())
    }

    async fn stdin_loop(key: Key, mut sender: Sender<(Key, Result<Text, std::io::Error>)>) -> Result<()> {
        use async_std::io::BufReader;
        use async_std::io;
        use async_std::prelude::*;
        use futures::SinkExt;

        let stdin = io::stdin();
        let mut lines = BufReader::new(stdin).lines();

        while let Some(line) = lines.next().await {
            match line {
                Ok(s) => sender.send((key, Ok(s))).await?,
                Err(err) => {
                    sender.send((key, Err(err))).await?;
                    break;
                }
            }
        }

        Ok(())
    }

    async fn run_loop(&mut self) -> Result<()> {
        use async_std::stream::StreamExt;
        let matchers = Matchers {
            match_pairs: &self.match_pairs,
            regex_set: &self.regex_set,
        };

        if !self.opt.debug {
            println!("{}", termion::cursor::Hide);
            println!("{}", termion::clear::All);
        }

        while let Some((key, item)) = self.receiver.next().await {
            if let Ok(s) = item {
                let program = &mut self.programs[key];
                program.append_line(s, &matchers);
            }

            if !self.opt.debug {
                self.redraw(DrawMode::Ongoing)?;
            }

            if unsafe { INTERRUPTED } {
                break;
            }

            if self.opt.interline_delay > 0 {
                std::thread::sleep(std::time::Duration::from_millis(self.opt.interline_delay as u64));
            }
        }

        if !self.opt.debug {
            self.redraw(DrawMode::Final)?;
            println!("{}", termion::cursor::Show);
        }

        Ok(())
    }

    fn redraw(&self, draw_mode: DrawMode) -> Result<()> {
         let (cx, cy) = termion::terminal_size()?;

         let cy = cy - match draw_mode {
             DrawMode::Final => self.opt.final_shrink as u16,
             DrawMode::Ongoing => 0,
         };

         let mut descriptions = vec![];

         print!("{}", termion::cursor::Goto(1, 1));

         for (_, program) in &self.programs {
             descriptions.push(program.calc_display_description(cx as usize, 0));
         }

         let mut total_lines = 0;
         for description in &descriptions {
             total_lines += description.lines().len();
         }

         let l = descriptions.len();
         if total_lines > cy as usize {
             for (idx, description) in descriptions.iter_mut().enumerate() {
                 let max = most_equal_divide(cy as u64, l as u64, idx as u64);
                 description.reduce_to_count(max as usize);
             }
         } else if total_lines < cy as usize {
             let extra = cy as usize - total_lines;

             descriptions.clear();
             for (idx, (_, program)) in self.programs.iter().enumerate() {
                 let added = most_equal_divide(extra as u64, l as u64, idx as u64);
                 descriptions.push(program.calc_display_description(cx as usize,
                         added as usize));
             }
         }

         let mut stdout = stdout();

         let mut line_idx = 0;
         for description in descriptions.iter() {
             for line in description.lines() {
                 match line.kind {
                     DisplayKind::MiddleTextCut(true) |
                     DisplayKind::Text(true) => {
                         write!(stdout, "{}{}",
                             termion::style::Bold,
                             termion::color::Fg(termion::color::Cyan))?;
                     }
                     _ => {}
                 }

                 write!(stdout, "{}{:>width$}{}{}",
                     termion::style::Bold, "", line.prefix,
                     termion::style::Reset, width = line.indent)?;

                 match line.kind {
                     DisplayKind::ProgramTitle |
                     DisplayKind::Title(true) => {
                         write!(stdout, "{}{}",
                             termion::style::Bold,
                             termion::color::Fg(termion::color::Cyan))?;
                     }
                     _ => {}
                 }

                 for fragment in line.text.iter() {
                     write!(stdout, "{}", fragment)?;
                 }

                 match line.kind {
                     DisplayKind::ProgramTitle |
                     DisplayKind::Title(true) => {
                         write!(stdout, "{}", termion::style::Reset)?;
                     }
                     _ => {}
                 }

                 line_idx += 1;

                 if line_idx == cy {
                     write!(stdout, "{}", termion::clear::UntilNewline)?;
                 } else {
                     writeln!(stdout, "{}", termion::clear::UntilNewline)?;
                 }
             }
         }
         write!(stdout, "{}", termion::clear::AfterCursor)?;
         stdout.flush()?;

         Ok(())
    }

    fn end_emit_output(&self, output: &Output, indent: usize) {
        match output {
            Output::Line(text) => {
                if self.opt.debug {
                    print!("{:>width$}", "", width = indent);
                    println!("Line: {}", text);
                } else {
                    println!("{}", text);
                }
            }
            Output::Encapsulation(encapsulation) => {
                if self.opt.debug {
                    print!("{:>width$}", "", width = indent);
                    println!("StartLine: {}", encapsulation.start_line);
                    print!("{:>width$}", "", width = indent);
                    println!("StartTitle: {}", encapsulation.start_title);
                } else {
                    println!("{}", encapsulation.start_line);
                }
                for output in &encapsulation.content {
                    self.end_emit_output(output, indent + 4);
                }

                if self.opt.debug {
                    print!("{:>width$}", "", width = indent);
                    println!("EndLine: {:?}", encapsulation.end_line);
                    print!("{:>width$}", "", width = indent);
                    println!("EndTitle: {:?}", encapsulation.end_title);
                } else {
                    if let Some(end_line) = &encapsulation.end_line {
                        println!("{}", end_line);
                    }
                }
            }
        }
    }

    fn end_execution(&mut self) -> Result<()> {
        for (_, program) in &self.programs {
            for output in program.content() {
                self.end_emit_output(&output, 0);
            }
        }

        Ok(())
    }
}

static mut INTERRUPTED : bool = false;

fn set_ctrl_c_handler() -> Result<()> {
    ctrlc::set_handler(move || {
        unsafe {
            if INTERRUPTED {
                std::process::exit(-1);
            }
            INTERRUPTED = true;
        }
    })?;

    Ok(())
}

/// No need for too many OS pthreads. Have the minimum that std async allows, as
/// we are doing most processing in the main thread anyway.
fn init_async() {
    let var = "ASYNC_STD_THREAD_COUNT";
    let prev = std::env::var(var);
    std::env::set_var(var, "1");
    async_std::task::block_on(async {});
    match prev {
        Err(_) => {
            std::env::remove_var(var);
        }
        Ok(x) => {
            std::env::set_var(var, x);
        }
    }
}

fn main() -> Result<()> {
    set_ctrl_c_handler()?;
    init_async();
    Main::new().run()
}
