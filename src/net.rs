use crate::game::{Action, GameState};
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::mpsc::Sender;
use std::thread;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Msg {
    Action(Action),
    State(GameState),
}

pub enum Event {
    Line(String),
    Net(Msg),
    Disconnected,
}

pub fn send(stream: &mut TcpStream, msg: &Msg) -> io::Result<()> {
    let mut line = serde_json::to_string(msg)?;
    line.push('\n');
    stream.write_all(line.as_bytes())
}

/// Reads newline-delimited JSON off `stream` until EOF/error, then emits Disconnected.
pub fn spawn_reader(stream: TcpStream, tx: Sender<Event>) {
    thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            match serde_json::from_str::<Msg>(&line) {
                Ok(msg) => {
                    if tx.send(Event::Net(msg)).is_err() {
                        return;
                    }
                }
                Err(_) => break, // ponytail: malformed line = drop the peer
            }
        }
        let _ = tx.send(Event::Disconnected);
    });
}

/// One event per line typed on stdin.
pub fn spawn_input(tx: Sender<Event>) {
    thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            let Ok(line) = line else { return };
            if tx.send(Event::Line(line)).is_err() {
                return;
            }
        }
    });
}
