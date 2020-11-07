#![recursion_limit = "256"]
use anyhow::Result;
use futures::FutureExt;
use futures::SinkExt;
use lazy_static::lazy_static;
use regex::{Regex, RegexSet};
use slab::Slab;
use std::fs::File;
use std::io::{stdout, BufRead, BufWriter, Stdout, Write};
use structopt::StructOpt;
use termion::screen::AlternateScreen;
use thiserror::Error;

mod cmdline;
mod display;
mod program;
mod util;

use display::DisplayKind;
use futures::channel::mpsc;
use program::Program;
use util::most_equal_divide;

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

    #[error("Unpaired Rege in file {0}. (Odd number of lines in it?)")]
    UnpairedRegexInFile(String),

    #[error("No programs specified")]
    NoPrograms,
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
    Lines(Vec<Text>),
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

        let a: &[&String] = &[];

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
                    found = true;
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

        if let Some(match_pairs_file) = &self.opt.match_pairs_file {
            let mut start = None;

            for line in std::io::BufReader::new(File::open(match_pairs_file)?).lines() {
                if start.is_none() {
                    start = Some(line?);
                    continue;
                }

                let start = Self::regex(&start.take().unwrap())?;
                let end = Self::regex(&line?)?;
                let pair = MatchPair { start, end };
                regex_set.push(String::from(pair.start.as_str()));
                regex_set.push(String::from(pair.end.as_str()));
                self.match_pairs.push(pair);
            }

            if let Some(start) = start {
                return Err(Error::UnpairedRegexInFile(start).into());
            }
        }

        self.regex_set = RegexSet::new(&regex_set)?;

        self.load_programs()?;

        if self.programs.is_empty() {
            if self.opt.programs_file.is_none() {
                self.insert_stdin()?;
            } else {
                return Err(Error::NoPrograms.into());
            }
        }

        drop(self.sender.take());

        if !self.opt.replay || self.opt.debug {
            async_std::task::block_on(async {
                let _ = self.run_loop().await;
            });
        } else {
            {
                let mut screen = AlternateScreen::from(stdout());
                async_std::task::block_on(async {
                    let _ = self.run_loop().await;
                });
                screen.flush().unwrap();
            }

            self.end_execution()?;
        }

        if self.opt.debug {
            self.end_execution()?;
        }

        Ok(())
    }

    fn add_child_program(&mut self, desc: String, mut child: std::process::Child) -> Result<()> {
        let stderr = child.stderr.take().unwrap();
        let stdout = child.stdout.take().unwrap();

        let (stderr, stdout) = unsafe {
            use std::os::unix::io::FromRawFd;
            use std::os::unix::io::IntoRawFd;
            let stderr = stderr.into_raw_fd();
            let stdout = stdout.into_raw_fd();
            let stderr = async_std::fs::File::from_raw_fd(stderr);
            let stdout = async_std::fs::File::from_raw_fd(stdout);
            (stderr, stdout)
        };

        let entry = self.programs.vacant_entry();
        let key = entry.key();
        let mut shutdown_senders = vec![];

        let (_shutdown_sender, shutdown_receiver) = mpsc::unbounded::<()>();
        shutdown_senders.push(_shutdown_sender);
        let broker_sender = self.sender.clone().unwrap();
        async_std::task::spawn(async move {
            let _res = Self::read_loop(key, broker_sender, shutdown_receiver, stdout).await;
        });

        let (_shutdown_sender, shutdown_receiver) = mpsc::unbounded::<()>();
        shutdown_senders.push(_shutdown_sender);
        let broker_sender = self.sender.clone().unwrap();
        async_std::task::spawn(async move {
            let _res = Self::read_loop(key, broker_sender, shutdown_receiver, stderr).await;
        });

        entry.insert(Program::new(desc, shutdown_senders).with_child(child));
        Ok(())
    }

    fn load_programs(&mut self) -> Result<()> {
        let std = "/bin/sh".to_owned();
        let shell = self.opt.shell.clone().unwrap_or(std);

        if let Some(pathname) = &self.opt.programs_file {
            let mut lines = vec![];

            if pathname == "-" {
                for line in std::io::BufReader::new(std::io::stdin()).lines() {
                    lines.push(line);
                }
            } else {
                let file = File::open(pathname)?;
                for line in std::io::BufReader::new(file).lines() {
                    lines.push(line);
                }
            };

            for line in lines.drain(..) {
                let line = line?;
                let child = std::process::Command::new(shell.clone())
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .arg("-c")
                    .arg(&line)
                    .spawn()?;
                self.add_child_program(line, child)?;
            }
        }

        lazy_static! {
            static ref RE: Regex = Regex::new("^-([/]+)-$").unwrap();
        }

        let mut next_cmd = vec![];
        let mut cmnds = vec![];

        for arg in &self.opt.programs {
            if let Some(r) = RE.captures(&arg) {
                let length = r.get(1).unwrap().as_str().len();
                if length == 1 {
                    cmnds.push(std::mem::replace(&mut next_cmd, vec![]));
                    continue;
                }

                let arg = (0..length - 1).map(|_| "/").collect::<String>();
                next_cmd.push(format!("-{}-", arg));
            } else {
                next_cmd.push(arg.to_owned());
            }
        }

        if !next_cmd.is_empty() {
            cmnds.push(next_cmd);
        }

        for cmnd in cmnds.drain(..) {
            let child = std::process::Command::new(&cmnd[0])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .args(&cmnd[1..])
                .spawn()?;

            use itertools::Itertools;
            let mut vec = cmnd.iter().map(|s| shell_escape::escape(s.as_str().into()));
            self.add_child_program(vec.join(" "), child)?;
        }

        Ok(())
    }

    fn insert_stdin(&mut self) -> Result<()> {
        let entry = self.programs.vacant_entry();
        let key = entry.key();
        let broker_sender = self.sender.clone().unwrap();
        let (_shutdown_sender, shutdown_receiver) = mpsc::unbounded::<()>();
        let mut shutdown_senders = vec![];

        async_std::task::spawn(async move {
            let _res = Self::read_loop(
                key,
                broker_sender,
                shutdown_receiver,
                async_std::io::stdin(),
            )
            .await;
        });

        shutdown_senders.push(_shutdown_sender);
        entry.insert(Program::new("<<stdin>>".to_owned(), shutdown_senders));

        Ok(())
    }

    async fn read_loop<R>(
        key: Key,
        mut sender: Sender<(Key, Result<Text, std::io::Error>)>,
        mut receiver: Receiver<()>,
        reader: R,
    ) -> Result<()>
    where
        R: futures::AsyncRead + Unpin,
    {
        use async_std::io::BufReader;
        use async_std::prelude::*;

        let mut lines = BufReader::new(reader).lines();

        loop {
            futures::select! {
                line = lines.next().fuse() => match line {
                    Some(Ok(s)) => sender.send((key, Ok(s))).await?,
                    Some(Err(err)) => {
                        sender.send((key, Err(err))).await?;
                        break;
                    }
                    None => break,
                },
                shutdown = receiver.next().fuse() => match shutdown {
                    Some(_) => break,
                    None => { }
                },
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

        let ctrlc = async_ctrlc::CtrlC::new().expect("cannot create Ctrl+C handler?");
        let mut ctrlc_stream = ctrlc.enumerate().take(3);
        let mut stdout = BufWriter::with_capacity(0x10000, stdout());
        let mut last_redraw_time = std::time::Instant::now();
        let mut need_redraw = false;
        let min_refresh_time = std::time::Duration::from_millis(4);

        loop {
            let never = async_std::future::pending::<()>();
            let dur = if need_redraw {
                min_refresh_time
            } else {
                std::time::Duration::from_millis(1000)
            };

            futures::select! {
                timeout = async_std::future::timeout(dur, never).fuse() => {
                    let now = std::time::Instant::now();
                    if last_redraw_time + min_refresh_time <= now {
                        self.redraw(DrawMode::Ongoing, &mut stdout)?;
                        last_redraw_time = now;
                        need_redraw = false
                    }
                },
                r = self.receiver.next().fuse() => match r {
                    Some((key, item)) => {
                        if let Ok(s) = item {
                            let program = &mut self.programs[key];
                            program.append_line(s, &matchers);
                        }

                        if !self.opt.debug {
                            let now = std::time::Instant::now();
                            if last_redraw_time + min_refresh_time <= now {
                                self.redraw(DrawMode::Ongoing, &mut stdout)?;
                                last_redraw_time = now;
                            } else {
                                need_redraw = true;
                            }
                        }

                        if self.opt.interline_delay > 0 {
                            async_std::task::sleep(std::time::Duration::from_millis(
                                    self.opt.interline_delay as u64,
                            )).await;
                        }
                    },
                    None => break,
                },
                ctrlc = ctrlc_stream.next().fuse() => match ctrlc {
                    Some(_) => break,
                    None => { }
                },
            }
        }

        if !self.opt.debug {
            self.redraw(DrawMode::Final, &mut stdout)?;
            println!("{}", termion::cursor::Show);
        }

        for (_, program) in &mut self.programs {
            program.shutdown().await;
        }

        Ok(())
    }

    fn redraw(&self, draw_mode: DrawMode, stdout: &mut BufWriter<Stdout>) -> Result<()> {
        let (cx, cy) = termion::terminal_size()?;

        let cy = cy
            - match draw_mode {
                DrawMode::Final => self.opt.final_shrink as u16,
                DrawMode::Ongoing => 0,
            };

        let mut descriptions = vec![];

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
                descriptions.push(program.calc_display_description(cx as usize, added as usize));
            }
        }

        write!(stdout, "{}", termion::cursor::Goto(1, 1))?;

        let mut line_idx = 0;
        for description in descriptions.iter() {
            for line in description.lines() {
                match line.kind {
                    DisplayKind::MiddleTextCut(true) | DisplayKind::Text(true) => {
                        write!(
                            stdout,
                            "{}{}",
                            termion::style::Bold,
                            termion::color::Fg(termion::color::Cyan)
                        )?;
                    }
                    _ => {}
                }

                write!(
                    stdout,
                    "{}{:>width$}{}{}",
                    termion::style::Bold,
                    "",
                    line.prefix,
                    termion::style::Reset,
                    width = line.indent
                )?;

                match line.kind {
                    DisplayKind::ProgramTitle | DisplayKind::Title(true) => {
                        write!(
                            stdout,
                            "{}{}",
                            termion::style::Bold,
                            termion::color::Fg(termion::color::Cyan)
                        )?;
                    }
                    _ => {}
                }

                for fragment in line.text.iter() {
                    write!(stdout, "{}", fragment)?;
                }

                match line.kind {
                    DisplayKind::ProgramTitle | DisplayKind::Title(true) => {
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
            Output::Lines(text) => {
                for text in text {
                    if self.opt.debug {
                        print!("{:>width$}", "", width = indent);
                        println!("Line: {}", text);
                    } else {
                        println!("{}", text);
                    }
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
    init_async();
    Main::new().run()
}
