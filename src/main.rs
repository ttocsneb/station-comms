use std::{
    sync::{Arc, Mutex},
    thread::{self, sleep},
    time::Duration,
};

use color_eyre::{install, Result};
use ordoo::or_do;
use rppal::uart::{Parity, Uart};
use sensor::Sensors;

use crate::stream::{MessageQueue, Reader, Rule};

#[macro_use]
extern crate lazy_static;

mod sensor;
mod stream;

fn main() -> Result<()> {
    install()?;

    let uart = Uart::with_path("/dev/serial0", 115200, Parity::None, 8, 1)?;
    let uart = Arc::new(Mutex::new(uart));
    let sensors = Arc::new(Mutex::new(Sensors::new()));
    let queue = Arc::new(MessageQueue::new());

    let mut reader = Reader::new(uart.clone());
    reader.on_err(|err| println!("{err}"));

    let q = queue.clone();
    reader.callback(Rule::default().letter(b'O').number(1), move |code| {
        // println!("Got finished code");
        if let Some(c) = code.params.first() {
            // println!("Received {}{}", c.letter as char, c.value,);
            q.send(c.letter, or_do!(c.value.as_borrowed().cast_u8(), s => s[0]));
            true
        } else {
            false
        }
    });

    let s = sensors.clone();
    reader.callback(Rule::default().letter(b'S'), move |code| {
        // println!("Got sensor code");
        let mut s = s.lock().unwrap();
        s.put(code);
        true
    });
    let s = sensors.clone();
    reader.callback(Rule::default().letter(b'M').number(102), move |code| {
        // println!("Got autos code");
        let mut val: i64 = 0;
        for p in &code.params {
            match p.letter {
                b'E' => {
                    let v = p.value.as_borrowed().cast_u8().unwrap();
                    val |= 1 << v
                }
                b'D' => {
                    let v = p.value.as_borrowed().cast_u8().unwrap();
                    val &= !(1 << v)
                }
                b'V' => {
                    let v = p.value.as_borrowed().cast_i64().unwrap();
                    val = v
                }
                _ => unreachable!(),
            }
        }
        let mut s = s.lock().unwrap();
        for sensor in s.iter_mut() {
            sensor.auto = (val & (1 << sensor.id)) != 0;
        }
        true
    });

    thread::spawn(move || reader.read());

    let to_do: [Box<dyn Fn() -> Result<_, _>>; 3] = [
        Box::new(|| stream::set_clock(uart.clone(), None)),
        Box::new(|| stream::request_all_sensors(uart.clone(), None)),
        Box::new(|| stream::request_autos(uart.clone(), None)),
    ];
    queue.retry_all(to_do)?;
    println!("{}", sensors.lock().unwrap());

    loop {
        sleep(Duration::from_millis(2500));

        let mut commands = Vec::new();
        for sensor in sensors.lock().unwrap().iter() {
            if sensor.auto {
                continue;
            }
            // println!("Getting {} {}", sensor.id, sensor.name);
            let u = uart.clone();
            let id = sensor.id;
            commands.push(Box::new(move || {
                stream::request_sensor(u.clone(), id, true, None)
            }));
        }

        queue.retry_all(commands)?;
        println!("{}", sensors.lock().unwrap());
    }
}
