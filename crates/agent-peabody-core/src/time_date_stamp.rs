// SPDX-FileCopyrightText: 2026 Gary Frattarola <garyf@parkviewlab.ai>
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The time-date stamp of a conversation entry: when it was made, in the local time
//! zone, as `Mon 2 Nov 2026, 14:01 PST`. The chat API has no field for it, so a request
//! carries it as a prefix to the entry's text, in square brackets; a model that copies
//! the habit into its own reply has the copy removed before the reply is kept or spoken.

use jiff::Zoned;

/// The time-date stamp of this moment.
pub fn now() -> String {
    Zoned::now().strftime("%a %-d %b %Y, %H:%M %Z").to_string()
}

/// The text of an entry as a request carries it: the stamp in brackets, then the text.
pub fn prefix(stamp: &str, text: &str) -> String {
    format!("[{stamp}] {text}")
}

/// A reply without the stamp a model put at its start, however many it put there.
pub fn strip(reply: &str) -> &str {
    let mut rest = reply.trim_start();
    while let Some(inner) = rest.strip_prefix('[')
        && let Some((stamp, after)) = inner.split_once(']')
        && is_stamp(stamp)
    {
        rest = after.trim_start();
    }
    rest
}

/// Whether `s` is shaped as `now` writes it: `Mon 2 Nov 2026, 14:01 PST`.
fn is_stamp(s: &str) -> bool {
    let Some((date, time)) = s.split_once(", ") else {
        return false;
    };
    let d: Vec<&str> = date.split(' ').collect();
    let t: Vec<&str> = time.split(' ').collect();
    let word = |w: &str| w.len() == 3 && w.chars().all(|c| c.is_ascii_alphabetic());
    let digits = |w: &str, n: std::ops::RangeInclusive<usize>| {
        n.contains(&w.len()) && w.chars().all(|c| c.is_ascii_digit())
    };
    let clock = |w: &str| {
        w.split_once(':')
            .is_some_and(|(h, m)| digits(h, 2..=2) && digits(m, 2..=2))
    };
    d.len() == 4
        && word(d[0])
        && digits(d[1], 1..=2)
        && word(d[2])
        && digits(d[3], 4..=4)
        && t.len() == 2
        && clock(t[0])
        && !t[1].is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_is_written_in_the_shape_strip_recognises() {
        let n = now();
        assert!(is_stamp(&n), "{n}");
        assert_eq!(strip(&prefix(&n, "It is two.")), "It is two.");
    }

    #[test]
    fn strip_removes_every_leading_stamp_and_nothing_else() {
        let a = "[Mon 2 Nov 2026, 14:01 PST]";
        assert_eq!(strip(&format!("{a} {a} Two o'clock.")), "Two o'clock.");
        assert_eq!(strip("Two o'clock."), "Two o'clock.");
        assert_eq!(strip("[aside] Two o'clock."), "[aside] Two o'clock.");
        assert_eq!(strip(&format!("It is {a}.")), format!("It is {a}."));
        assert_eq!(
            strip("[Mon 2 Nov 2026, 14:01 PST"),
            "[Mon 2 Nov 2026, 14:01 PST"
        );
    }
}
