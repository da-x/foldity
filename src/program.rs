use super::display::{DisplayDescription, DisplayKind, DisplayLine};
use super::{Encapsulation, Matchers, Output, PairId, Text};
use smallvec::SmallVec;

pub struct Program {
    desc: String,
    content: Vec<Output>,
}

impl Program {
    pub(crate) fn content(&self) -> &Vec<Output> {
        &self.content
    }

    pub fn new(desc: String) -> Self {
        Self {
            desc,
            content: vec![],
        }
    }

    pub(crate) fn append_line(&mut self, s: Text, matchers: &Matchers<'_>) {
        enum Side {
            Start,
            End,
        };
        let mut encapsulation = None;
        if matchers.regex_set.is_match(&s) {
            for (pair_id, pair) in matchers.match_pairs.iter().enumerate() {
                if let Some(captures) = pair.start.captures(&s) {
                    encapsulation = Some((pair_id, Side::Start, captures));
                    break;
                }
                if let Some(captures) = pair.end.captures(&s) {
                    encapsulation = Some((pair_id, Side::End, captures));
                    break;
                }
            }
        }

        if let Some((pair_id, side, captures)) = encapsulation {
            let title = if captures.len() > 2 {
                match captures.name("M") {
                    None => String::new(),
                    Some(x) => String::from(x.as_str()),
                }
            } else {
                String::from(captures.get(1).unwrap().as_str())
            };
            match side {
                Side::Start => {
                    let encapsulation = Encapsulation {
                        start_title: title,
                        pair_id,
                        start_line: s,
                        end_line: None,
                        end_title: None,
                        content: vec![],
                    };
                    Self::push_regular(&mut self.content, Output::Encapsulation(encapsulation));
                }
                Side::End => {
                    let _ = Self::push_end(&mut self.content, (title, s, pair_id));
                }
            }
        } else {
            Self::push_regular(&mut self.content, Output::Line(s));
        }
    }

    fn push_end(
        content: &mut Vec<Output>,
        s: (String, String, PairId),
    ) -> Option<(String, String, PairId)> {
        if let Some(last) = content.last_mut() {
            match last {
                Output::Line(_) => Some(s),
                Output::Encapsulation(encapsulation) => {
                    if encapsulation.is_ended() {
                        return Some(s);
                    } else {
                        if let Some((title, s, _)) = Self::push_end(&mut encapsulation.content, s) {
                            encapsulation.end_line = Some(s);
                            encapsulation.end_title = Some(title);
                        }
                        None
                    }
                }
            }
        } else {
            Some(s)
        }
    }

    fn push_regular(content: &mut Vec<Output>, s: Output) {
        if let Some(last) = content.last_mut() {
            match last {
                Output::Line(_) => {
                    content.push(s);
                }
                Output::Encapsulation(encapsulation) => {
                    if encapsulation.is_ended() {
                        content.push(s);
                    } else {
                        Self::push_regular(&mut encapsulation.content, s);
                    }
                }
            }
        } else {
            content.push(s);
        }
    }

    pub fn calc_display_description<'a>(
        &'a self,
        cx: usize,
        allowed_extra: usize,
    ) -> DisplayDescription<'a> {
        let mut dd = DisplayDescription::new(cx);

        dd.add_line(DisplayLine {
            indent: 0,
            kind: DisplayKind::ProgramTitle,
            prefix: "",
            text: SmallVec::from(&[self.desc.as_str().into()][..]),
        });

        dd.add_content(&self.content, 0, allowed_extra, true);

        dd
    }
}
