//! Parsing of PoE2's `Client.txt` log lines into game events. The log is
//! plain text the client writes live (zone changes, whispers, area joins),
//! so tailing it needs no game-memory reading. Each line looks like:
//!
//! `2026/07/24 12:18:52 313944271 3ef231e0 [INFO Client 356] : <message>`
//!
//! Everything after the first `] : ` is the message; this module classifies
//! that message. Unrecognized lines return `None` and are skipped.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogEvent {
    /// The local player entered a zone (the trailing period is stripped).
    ZoneEnter(String),
    /// An incoming whisper: the sender and the message body.
    WhisperFrom { from: String, message: String },
    /// A character joined the local player's area (party/trade arrival).
    AreaJoin(String),
}

/// Extracts the message body after the `] : ` marker, if present.
fn message_body(line: &str) -> Option<&str> {
    line.split_once("] : ").map(|(_, msg)| msg.trim_end())
}

/// Classifies one raw log line into a `LogEvent`, or `None` if it is not one
/// of the recognized events.
pub fn parse_line(line: &str) -> Option<LogEvent> {
    let msg = message_body(line)?;
    if let Some(rest) = msg.strip_prefix("@From ") {
        // "@From <Name>: <message>" (a leading guild tag like "<TAG> " may
        // precede the name; the name is up to the first ": ").
        let (from, message) = rest.split_once(": ")?;
        return Some(LogEvent::WhisperFrom {
            from: from.trim().to_string(),
            message: message.to_string(),
        });
    }
    if let Some(zone) = msg.strip_prefix("You have entered ") {
        return Some(LogEvent::ZoneEnter(zone.trim_end_matches('.').to_string()));
    }
    if let Some(name) = msg.strip_suffix(" has joined the area.") {
        return Some(LogEvent::AreaJoin(name.to_string()));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREFIX: &str = "2026/07/24 12:18:52 313944271 3ef231e0 [INFO Client 356] : ";

    #[test]
    fn parses_zone_enter() {
        let line = format!("{PREFIX}You have entered Ziggurat Refuge.");
        assert_eq!(
            parse_line(&line),
            Some(LogEvent::ZoneEnter("Ziggurat Refuge".to_string()))
        );
    }

    #[test]
    fn parses_incoming_whisper() {
        let line = format!("{PREFIX}@From Xyz123: wtb your waystone 1 exalt");
        assert_eq!(
            parse_line(&line),
            Some(LogEvent::WhisperFrom {
                from: "Xyz123".to_string(),
                message: "wtb your waystone 1 exalt".to_string(),
            })
        );
    }

    #[test]
    fn parses_area_join() {
        let line = format!("{PREFIX}HassanHM has joined the area.");
        assert_eq!(
            parse_line(&line),
            Some(LogEvent::AreaJoin("HassanHM".to_string()))
        );
    }

    #[test]
    fn ignores_other_lines() {
        assert_eq!(parse_line(&format!("{PREFIX}: Connecting to instance server")), None);
        assert_eq!(parse_line("garbage without marker"), None);
    }
}
