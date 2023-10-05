use chrono::{NaiveDate, NaiveTime};
use color_eyre::Result;
use ordoo::or_do;
use std::{
    sync::{
        mpsc::{Receiver, Sender},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use rppal::uart::Uart;
use scode_rs::{error::ScodeError, Code, CodeSend, CodeStream, ParamSend, ParamValue};

#[derive(Debug, Default)]
pub struct Rule {
    pub letter: Option<u8>,
    pub number: Option<u8>,
}

impl Rule {
    pub fn new(letter: u8, number: u8) -> Self {
        Self {
            letter: Some(letter),
            number: Some(number),
        }
    }

    pub fn letter(letter: u8) -> Self {
        Self {
            letter: Some(letter),
            number: None,
        }
    }
    pub fn number(number: u8) -> Self {
        Self {
            letter: None,
            number: Some(number),
        }
    }

    pub fn matches(&self, code: &CodeSend) -> bool {
        if let Some(letter) = self.letter {
            if letter != code.letter {
                return false;
            }
        }
        if let Some(number) = self.number {
            if number != code.number {
                return false;
            }
        }
        true
    }
}

pub struct StationReader<T> {
    uart: Uart,
    on_recv: Sender<T>,
    on_send: Receiver<CodeSend>,
    last_send: Instant,
    bytes_sent: isize,
    to_send: Vec<u8>,
}

impl<T> StationReader<T>
where
    T: From<CodeSend> + From<ScodeError>,
{
    pub fn new(uart: Uart, on_recv: Sender<T>, on_send: Receiver<CodeSend>) -> Self {
        Self {
            uart,
            on_recv,
            on_send,
            last_send: Instant::now(),
            bytes_sent: 0,
            to_send: Vec::new(),
        }
    }

    /// The main loop for communicating with the station sensors
    ///
    /// This will poll the station every 150ms
    pub fn main(&mut self) -> Result<()> {
        self.uart.set_read_mode(0, Duration::ZERO)?;
        let mut stream = CodeStream::with_capacity(64);

        let mut buf = [0; 64];
        loop {
            let len = self.uart.read(&mut buf)?;
            if len == 0 {
                match self.on_send.recv_timeout(Duration::from_millis(150)) {
                    Ok(to_send) => {
                        let code = Code::try_from(to_send)?;
                        let mut buf = code.dump_binary_vec()?;
                        self.to_send.append(&mut buf);
                    }
                    Err(_) => {}
                }
                if !self.to_send.is_empty() {
                    let now = Instant::now();
                    if (now - self.last_send) > Duration::from_millis(150) {
                        self.last_send = Instant::now();
                        self.bytes_sent = 0;
                    }
                    let allotment = 60 - self.bytes_sent;
                    if allotment <= 0 {
                        continue;
                    }
                    let len = self.to_send.len().min(allotment as usize);
                    let to_send = &self.to_send[0..len];
                    self.uart.write(to_send)?;
                    self.to_send.drain(0..len);
                    self.bytes_sent += len as isize;
                }
                continue;
            }
            stream.extend(&buf[0..len]);
            for code in &mut stream {
                match code {
                    Ok(code) => {
                        let code = CodeSend::from(code);
                        self.on_recv.send(T::from(code)).unwrap();
                    }
                    Err(err) => {
                        self.on_recv.send(T::from(err)).unwrap();
                    }
                }
            }
        }
    }
}

pub struct CodeHandler {
    callbacks: Vec<(Rule, Box<dyn Fn(&CodeSend) -> bool + Send>)>,
}

impl CodeHandler {
    pub fn new() -> Self {
        Self {
            callbacks: Vec::new(),
        }
    }

    #[inline]
    pub fn callback<F>(&mut self, (r, cb): (Rule, F))
    where
        F: Fn(&CodeSend) -> bool + Send + 'static,
    {
        self.callbacks.push((r, Box::new(cb)));
    }

    pub fn code(&mut self, code: CodeSend) -> bool {
        for (_, f) in self.callbacks.iter().filter(|(r, _)| r.matches(&code)) {
            if f(&code) {
                return true;
            }
        }
        false
    }
}

struct Waiting {
    key: (u8, u8),
    code: CodeSend,
    due: Instant,
    retry: Duration,
    notify: Sender<(u8, u8)>,
}

pub struct CommandManager {
    waiting: Mutex<Vec<Waiting>>,
    tx: Mutex<Sender<CodeSend>>,
}

impl CommandManager {
    pub fn new(tx: Sender<CodeSend>) -> Self {
        Self {
            waiting: Mutex::new(Vec::new()),
            tx: Mutex::new(tx),
        }
    }

    pub fn on_command(self: &Arc<Self>) -> (Rule, impl Fn(&CodeSend) -> bool) {
        let s = self.clone();
        (Rule::new(b'O', 1), move |code| {
            let mut waiting = s.waiting.lock().unwrap();
            if let Some(c) = code.params.first() {
                let number = or_do!(c.value.as_borrowed().cast_u8(), _ => return false);
                if let Some((i, _)) = waiting
                    .iter()
                    .enumerate()
                    .find(|(_, v)| v.key == (c.letter, number))
                {
                    let v = waiting.swap_remove(i);
                    v.notify.send((c.letter, number)).unwrap();
                }
                true
            } else {
                false
            }
        })
    }

    pub fn command(&self, code: CodeSend) {
        self.tx.lock().unwrap().send(code).unwrap();
    }

    pub fn command_guarentee(&self, code: CodeSend, tx: Sender<(u8, u8)>, retry: Duration) {
        let mut waiting = self.waiting.lock().unwrap();
        self.tx.lock().unwrap().send(code.clone()).unwrap();
        waiting.push(Waiting {
            key: (code.letter, code.number),
            code,
            due: Instant::now() + retry,
            retry,
            notify: tx,
        });
    }

    pub fn update(&self) {
        let mut waiting = self.waiting.lock().unwrap();
        let now = Instant::now();

        for waiting in waiting.iter_mut().filter(|w| w.due <= now) {
            waiting.due = now + waiting.retry;
            self.tx.lock().unwrap().send(waiting.code.clone()).unwrap();
        }
    }

    pub fn earliest_due(&self) -> Option<Instant> {
        self.waiting.lock().unwrap().iter().map(|v| v.due).min()
    }
}

pub fn request_all_sensors_code() -> CodeSend {
    CodeSend {
        letter: b'M',
        number: 1,
        params: vec![],
    }
}

pub fn request_sensor_code(sensor: u8, only_value: bool) -> CodeSend {
    CodeSend {
        letter: b'S',
        number: sensor,
        params: match only_value {
            true => vec![ParamSend {
                letter: b'R',
                value: ParamValue::str("V"),
            }],
            false => vec![],
        },
    }
}

pub fn set_clock_code() -> CodeSend {
    let year = NaiveDate::from_yo_opt(1984, 1).unwrap();
    let midnight = NaiveTime::from_hms_opt(0, 0, 0).unwrap();

    let now = chrono::offset::Local::now();
    let days = (now.date_naive() - year).num_days();
    let ms = (now.time() - midnight).num_milliseconds();

    CodeSend {
        letter: b'M',
        number: 10,
        params: vec![
            ParamSend {
                letter: b'D',
                value: (days as i32).into(),
            },
            ParamSend {
                letter: b'T',
                value: (ms as i32).into(),
            },
        ],
    }
}

#[allow(dead_code)]
pub fn reset_code() -> CodeSend {
    CodeSend {
        letter: b'M',
        number: 20,
        params: vec![],
    }
}

pub fn request_autos_code() -> CodeSend {
    CodeSend {
        letter: b'M',
        number: 102,
        params: vec![],
    }
}
