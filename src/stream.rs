use chrono::{NaiveDate, NaiveTime};
use color_eyre::Result;
use ordoo::or_do;
use std::{
    sync::{Arc, Condvar, Mutex},
    thread::sleep,
    time::{Duration, Instant},
};

use rppal::uart::Uart;
use scode_rs::{error::ScodeError, Code, CodeSend, CodeStream, ParamSend, ParamValue};

fn print_buf(buf: &[u8]) {
    for c in buf {
        if *c >= b' ' && *c <= b'~' {
            print!("{}", *c as char);
        } else {
            print!("·")
        }
    }
    println!("");
}

pub struct MessageQueue {
    waiting: Mutex<Vec<(u8, u8)>>,
    receive: Mutex<Vec<(u8, u8)>>,
    notify: Condvar,
}

impl MessageQueue {
    pub fn new() -> Self {
        Self {
            waiting: Mutex::new(Vec::new()),
            receive: Mutex::new(Vec::new()),
            notify: Condvar::new(),
        }
    }

    pub fn try_wait(&self, letter: u8, number: u8) -> bool {
        let mut receive = self.receive.lock().unwrap();
        if let Some((i, _)) = receive
            .iter()
            .enumerate()
            .find(|(_, v)| **v == (letter, number))
        {
            receive.swap_remove(i);
            true
        } else {
            false
        }
    }

    pub fn wait_for(&self, letter: u8, number: u8) -> bool {
        if self.try_wait(letter, number) {
            return true;
        }

        self.waiting.lock().unwrap().push((letter, number));

        let (mut receive, timeout) = self
            .notify
            .wait_timeout_while(
                self.receive.lock().unwrap(),
                Duration::from_secs(1),
                |receive| receive.iter().find(|v| **v == (letter, number)).is_none(),
            )
            .unwrap();
        if timeout.timed_out() {
            return false;
        }

        let (i, _) = or_do!(
            receive
                .iter()
                .enumerate()
                .find(|(_, v)| **v == (letter, number)),
            return false
        );
        receive.swap_remove(i);

        true
    }

    pub fn wait_all<I>(&self, items: I) -> Vec<(u8, u8)>
    where
        I: IntoIterator<Item = (u8, u8)>,
    {
        let mut to_wait = vec![];
        let mut failed = Vec::new();
        for (l, n) in items {
            if !self.try_wait(l, n) {
                to_wait.push((l, n));
            }
        }
        let mut to_wait = to_wait.into_iter();
        loop {
            if let Some((l, n)) = to_wait.next() {
                if !self.wait_for(l, n) {
                    failed.push((l, n));
                    failed.append(&mut self.wait_all(to_wait));
                    break;
                }
            } else {
                break;
            }
        }
        failed
    }

    pub fn send(&self, letter: u8, number: u8) {
        self.receive.lock().unwrap().push((letter, number));

        let mut waiting = self.waiting.lock().unwrap();
        if let Some((i, _)) = waiting
            .iter()
            .enumerate()
            .find(|(_, v)| **v == (letter, number))
        {
            waiting.swap_remove(i);
            drop(waiting);
            self.notify.notify_one();
            return;
        }
        drop(waiting);
    }
    pub fn retry_all<I, F, R>(&self, items: I) -> Result<(), R>
    where
        I: IntoIterator<Item = F>,
        F: Fn() -> Result<(u8, u8), R>,
    {
        let mut to_wait = vec![];
        let mut map = vec![];
        for f in items {
            let v = f()?;
            to_wait.push(v);
            map.push((v, f));
        }

        // println!("{:?}", to_wait);

        let to_wait = self.wait_all(to_wait);
        let mut to_run = Vec::with_capacity(to_wait.len());
        for j in to_wait {
            if let Some((i, _)) = map.iter().enumerate().find(|(_, (k, _))| *k == j) {
                let ((l, n), f) = map.swap_remove(i);
                println!("Retrying {}{n}", l as char);
                to_run.push(f);
            }
        }

        if !to_run.is_empty() {
            self.retry_all(to_run)?;
        }
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct Rule {
    pub letter: Option<u8>,
    pub number: Option<u8>,
}

impl Rule {
    pub fn letter(mut self, letter: u8) -> Self {
        self.letter = Some(letter);
        self
    }
    pub fn number(mut self, number: u8) -> Self {
        self.number = Some(number);
        self
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

pub struct Reader {
    uart: Arc<Mutex<Uart>>,
    callbacks: Vec<(Rule, Box<dyn Fn(&CodeSend) -> bool + Send>)>,
    err: Option<Box<dyn Fn(ScodeError) + Send>>,
}

impl Reader {
    pub fn new(uart: Arc<Mutex<Uart>>) -> Self {
        Self {
            uart,
            callbacks: vec![],
            err: None,
        }
    }

    pub fn callback<F>(&mut self, rule: Rule, callback: F)
    where
        F: Fn(&CodeSend) -> bool + Send + 'static,
    {
        self.callbacks.push((rule, Box::new(callback)));
    }

    pub fn on_err<F>(&mut self, callback: F)
    where
        F: Fn(ScodeError) + Send + 'static,
    {
        self.err = Some(Box::new(callback));
    }

    pub fn read(&self) -> Result<()> {
        self.uart.lock().unwrap().set_read_mode(0, Duration::ZERO)?;
        let mut stream = CodeStream::with_capacity(64);

        let mut buf = [0; 64];
        loop {
            let len = self.uart.lock().unwrap().read(&mut buf)?;
            if len == 0 {
                sleep(Duration::from_millis(100));
                continue;
            }
            // print!("Recv {len}: ");
            // print_buf(&buf[0..len]);
            stream.extend(&buf[0..len]);
            for code in &mut stream {
                match code {
                    Ok(code) => {
                        // println!("Received {code}");
                        let code = CodeSend::from(code);
                        for (_, f) in self.callbacks.iter().filter(|(r, _)| r.matches(&code)) {
                            if f(&code) {
                                break;
                            }
                        }
                    }
                    Err(err) => {
                        if let Some(cb) = &self.err {
                            cb(err);
                        }
                    }
                }
            }
        }
    }
}

#[allow(dead_code)]
pub fn reader<F>(uart: Arc<Mutex<Uart>>, callback: F) -> Result<()>
where
    F: Fn(Result<CodeSend, ScodeError>) -> (),
{
    uart.lock().unwrap().set_read_mode(0, Duration::ZERO)?;
    let mut stream = CodeStream::with_capacity(64);

    let mut buf = [0; 64];
    loop {
        let len = uart.lock().unwrap().read(&mut buf)?;
        if len == 0 {
            sleep(Duration::from_millis(100));
            continue;
        }
        stream.extend(&buf[0..len]);

        for res in &mut stream {
            let res = res.map(|v| v.into());
            callback(res);
        }
    }
}

pub fn send(
    uart: Arc<Mutex<Uart>>,
    code: CodeSend,
    queue: Option<&MessageQueue>,
) -> Result<(u8, u8)> {
    lazy_static! {
        static ref COUNT: Arc<Mutex<(usize, Instant)>> = Arc::new(Mutex::new((0, Instant::now())));
    }

    let letter = code.letter;
    let number = code.number;

    let code = Code::try_from(code)?;
    let mut buf = [0; 64];
    let len = code.dump_binary(&mut buf)?;

    let mut count = COUNT.lock().unwrap();
    let now = Instant::now();
    if now - count.1 < Duration::from_millis(150) {
        count.0 += len;
        if count.0 >= 64 {
            count.0 = len;
            println!("Too many comms, sleeping for a bit");
            sleep(Duration::from_millis(150));
            count.1 = now;
        }
    } else {
        count.0 = len;
        count.1 = now;
    }

    if let Some(queue) = queue {
        loop {
            // print!("Send {len}: ");
            // print_buf(&buf[0..len]);
            uart.lock().unwrap().write(&buf[0..len])?;
            if queue.wait_for(letter, number) {
                break;
            }
        }
    } else {
        // print!("Send {len}: ");
        // print_buf(&buf[0..len]);
        uart.lock().unwrap().write(&buf[0..len])?;
    }
    Ok((letter, number))
}

pub fn request_all_sensors(
    uart: Arc<Mutex<Uart>>,
    queue: Option<&MessageQueue>,
) -> Result<(u8, u8)> {
    let code = CodeSend {
        letter: b'M',
        number: 1,
        params: vec![],
    };
    send(uart, code, queue)
}

pub fn request_sensor(
    uart: Arc<Mutex<Uart>>,
    sensor: u8,
    only_value: bool,
    queue: Option<&MessageQueue>,
) -> Result<(u8, u8)> {
    let code = CodeSend {
        letter: b'S',
        number: sensor,
        params: match only_value {
            true => vec![ParamSend {
                letter: b'R',
                value: ParamValue::str("V"),
            }],
            false => vec![],
        },
    };
    send(uart, code, queue)
}

pub fn set_clock(uart: Arc<Mutex<Uart>>, queue: Option<&MessageQueue>) -> Result<(u8, u8)> {
    let year = NaiveDate::from_yo_opt(1984, 1).unwrap();
    let midnight = NaiveTime::from_hms_opt(0, 0, 0).unwrap();

    let now = chrono::offset::Local::now();
    let days = (now.date_naive() - year).num_days();
    let ms = (now.time() - midnight).num_milliseconds();

    let code = CodeSend {
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
    };

    send(uart, code, queue)
}

pub fn reset(uart: Arc<Mutex<Uart>>, queue: Option<&MessageQueue>) -> Result<(u8, u8)> {
    let code = CodeSend {
        letter: b'M',
        number: 20,
        params: vec![],
    };
    send(uart, code, queue)
}

pub fn request_autos(uart: Arc<Mutex<Uart>>, queue: Option<&MessageQueue>) -> Result<(u8, u8)> {
    let code = CodeSend {
        letter: b'M',
        number: 102,
        params: vec![],
    };
    send(uart, code, queue)
}
