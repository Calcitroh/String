#[derive(Default)]
pub(crate) struct Feedback {
    pub logs: Vec<String>,
    pub messages: Vec<String>,
    pub reason: Option<String>,
    seen: std::collections::HashSet<String>,
    backup_warned: bool,
}

impl Feedback {
    pub fn backup(&mut self, reason: String) {
        self.log(&reason);
        if !self.backup_warned {
            self.messages.push(reason);
            self.backup_warned = true;
        }
    }

    pub fn saved(&mut self, errors: &[String]) {
        for reason in errors {
            self.log(reason);
        }
        match (&self.reason, errors.first()) {
            (None, Some(reason)) => self.messages.push(format!(
                "String can't save right now ({reason}). Your changes are kept, and it keeps trying."
            )),
            (Some(_), None) => self.messages.push("Your changes are saved again".into()),
            _ => {}
        }
        self.reason = (!errors.is_empty()).then(|| errors.join("\n"));
    }

    fn log(&mut self, reason: &str) {
        if self.seen.insert(reason.to_owned()) {
            self.logs.push(reason.to_owned());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_saves_announce_once_keep_the_current_reason_and_announce_recovery() {
        let mut feedback = Feedback::default();
        for reason in ["disk full", "disk full", "file locked", "file locked"] {
            feedback.saved(&[reason.into()]);
            assert_eq!(feedback.reason.as_deref(), Some(reason));
        }
        feedback.saved(&[]);
        feedback.saved(&[]);
        assert_eq!(feedback.logs, ["disk full", "file locked"]);
        assert_eq!(feedback.messages.len(), 2);
        assert!(feedback.messages[0].contains("disk full"));
        assert_eq!(feedback.messages[1], "Your changes are saved again");
        assert_eq!(feedback.reason, None);
    }

    #[test]
    fn opening_and_daily_backup_errors_give_one_warning_and_log_each_reason_once() {
        let mut feedback = Feedback::default();
        for reason in [
            "opening copy failed",
            "daily copy failed",
            "daily copy failed",
        ] {
            feedback.backup(reason.into());
        }
        feedback.saved(&[]);
        assert_eq!(feedback.messages, ["opening copy failed"]);
        assert_eq!(feedback.logs, ["opening copy failed", "daily copy failed"]);
        assert_eq!(feedback.reason, None);
    }
}
