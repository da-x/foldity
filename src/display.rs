use super::Output;
use smallvec::SmallVec;

pub enum DisplayKind {
    ProgramTitle,
    Title(bool),
    Text(bool),
    MiddleTextCut(bool),
    WholeScreenCut,
}

pub struct DisplayLine<'a> {
    pub indent: usize,
    pub kind: DisplayKind,
    pub prefix: &'static str,
    pub text: SmallVec<[&'a str; 3]>,
}

pub struct DisplayDescription<'a> {
    cx: usize,
    lines: Vec<DisplayLine<'a>>,
}

impl<'a> DisplayDescription<'a> {
    pub fn new(cx: usize) -> Self {
        DisplayDescription { lines: vec![], cx }
    }

    pub fn lines(&self) -> &Vec<DisplayLine<'a>> {
        &self.lines
    }

    pub fn add_line(&mut self, mut dl: DisplayLine<'a>) {
        let total_indent = dl.indent + dl.prefix.len();
        let elipsis = "...";
        let cx_remain = self.cx - total_indent - elipsis.len();

        // Trim, but support wrapping in the future.

        let mut row_x = 0;
        let mut last_idx = None;
        let mut idx = 0;

        while idx < dl.text.len() {
            // Tab expansion
            if dl.text[idx].contains('\t') {
                let mut t = dl.text[idx];
                let mut new_row_x = row_x;
                let mut new_idx = idx;
                dl.text.remove(idx);

                while let Some(cpos) = t.find('\t') {
                    dl.text.insert(new_idx, &t[..cpos]);
                    dl.text
                        .insert(new_idx + 1, &"        "[..8 - (new_row_x % 8)]);
                    new_row_x += cpos;
                    t = &t[cpos + 1..];
                    new_idx += 2;
                }
                dl.text.insert(new_idx, &t[..]);
            }

            let fragment = &mut dl.text[idx];
            row_x += fragment.len();

            if row_x > cx_remain {
                let chunk = &fragment[..fragment.len() - (row_x - cx_remain)];
                *fragment = chunk;
                last_idx = Some(idx);
                break;
            }

            idx += 1;
        }

        if let Some(last_idx) = last_idx {
            dl.text.truncate(last_idx + 1);
            dl.text.push(elipsis.into());
        }

        self.lines.push(dl);
    }

    pub(crate) fn add_content(
        &mut self,
        content: &'a Vec<Output>,
        indent: usize,
        allowed_extra: usize,
        last: bool,
    ) {
        let n = content.len();
        let vertical = "⫼ ";
        let cut = "+-------------------------------------";

        for (idx, output) in content.iter().enumerate() {
            match output {
                Output::Encapsulation(encapsulation) => {
                    if let Some(end_title) = &encapsulation.end_title {
                        let mut text = SmallVec::new();
                        text.push(encapsulation.start_title.as_str().into());
                        if end_title.len() > 0 {
                            text.push(" ".into());
                            text.push(end_title.as_str().into());
                        }
                        self.add_line(DisplayLine {
                            indent,
                            kind: DisplayKind::Title(false),
                            prefix: "└── ",
                            text,
                        });
                    } else {
                        let mut text = SmallVec::new();
                        text.push(encapsulation.start_title.as_str().into());
                        self.add_line(DisplayLine {
                            indent,
                            kind: DisplayKind::Title(true),
                            prefix: "└── ",
                            text,
                        });
                        self.add_content(
                            &encapsulation.content,
                            indent + 4,
                            allowed_extra,
                            last && idx + 1 == n,
                        );
                    }
                }
                Output::Lines(lines) => {
                    let nr_lines = lines.len();

                    let minimum = 3;
                    let mut minimization_threshold = minimum;
                    let last_here = if idx + 1 == n {
                        // Nothing follows the lines, allow more regular lines
                        minimization_threshold += allowed_extra;
                        last
                    } else {
                        false
                    };

                    if nr_lines > minimization_threshold {
                        // First and last_here line
                        self.add_line(DisplayLine {
                            indent,
                            kind: DisplayKind::Text(last_here),
                            prefix: vertical,
                            text: SmallVec::from_elem(lines[0].as_str().into(), 1),
                        });
                        self.add_line(DisplayLine {
                            indent,
                            kind: DisplayKind::MiddleTextCut(last_here),
                            prefix: cut,
                            text: SmallVec::new(),
                        });
                        for x in nr_lines - 1 - (minimization_threshold - minimum)
                            ..nr_lines
                        {
                            self.add_line(DisplayLine {
                                indent,
                                kind: DisplayKind::Text(last_here),
                                prefix: vertical,
                                text: SmallVec::from_elem(lines[x].as_str().into(), 1),
                            });
                        }
                    } else {
                        // All lines
                        for line in lines {
                            self.add_line(DisplayLine {
                                indent,
                                kind: DisplayKind::Text(last_here),
                                prefix: vertical,
                                text: SmallVec::from_elem(line.as_str().into(), 1),
                            });
                        }
                    }
                }
            }
        }
    }

    pub fn reduce_to_count(&mut self, count: usize) {
        self.lines.drain(1..self.lines.len() - count + 1);
        self.lines.insert(
            1,
            DisplayLine {
                indent: 0,
                kind: DisplayKind::WholeScreenCut,
                prefix: "",
                text: SmallVec::new(),
            },
        );
    }
}
