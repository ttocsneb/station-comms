use ordoo::or_do;
use scode_rs::CodeSend;
use std::{
    collections::{btree_map, BTreeMap},
    fmt::Display,
    iter::Map,
    sync::Arc,
    time::Instant,
};

#[derive(Debug)]
pub struct Sensor {
    pub name: Arc<str>,
    pub unit: Arc<str>,
    pub id: u8,
    pub value: f32,
    pub last_update: Instant,
    pub auto: bool,
}

impl Display for Sensor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("{}: {} {}", self.name, self.value, self.unit))?;
        if self.auto {
            f.write_str(" [auto]")?;
        }
        Ok(())
    }
}

pub struct Sensors {
    sensors: BTreeMap<u8, Sensor>,
    map: BTreeMap<Arc<str>, u8>,
}

impl Display for Sensors {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[\n")?;
        for sensor in self.iter() {
            f.write_str("  ")?;
            sensor.fmt(f)?;
            f.write_str(",\n")?;
        }
        f.write_str("]")
    }
}

impl Sensors {
    pub fn new() -> Self {
        Self {
            sensors: BTreeMap::new(),
            map: BTreeMap::new(),
        }
    }

    pub fn put(&mut self, code: &CodeSend) -> bool {
        if code.letter != b'S' {
            return false;
        }
        let now = Instant::now();
        let value = or_do!(code.find(b'V'), return false);
        let value = or_do!(
            value.value.as_borrowed().cast_f32(),
            v => or_do!(
                std::str::from_utf8(v).ok().map(|v| v.parse().ok()).flatten(),
                return false
            )
        );

        if let Some(sensor) = self.sensors.get_mut(&code.number) {
            sensor.last_update = now;
            sensor.value = value;
        } else {
            let name = or_do!(code.find(b'N'), return false)
                .value
                .as_borrowed()
                .cast_bytes();
            let unit = or_do!(code.find(b'U'), return false)
                .value
                .as_borrowed()
                .cast_bytes();
            let sensor = Sensor {
                name: String::from_utf8_lossy(&name).into(),
                unit: String::from_utf8_lossy(&unit).into(),
                id: code.number,
                value,
                last_update: now,
                auto: false,
            };

            self.map.insert(sensor.name.clone(), sensor.id);
            self.sensors.insert(sensor.id, sensor);
        }
        true
    }

    pub fn get(&self, name: impl AsRef<str>) -> Option<&Sensor> {
        self.map
            .get(name.as_ref())
            .map(|v| self.sensors.get(&v))
            .flatten()
    }

    #[inline]
    pub fn iter(&self) -> <&Sensors as IntoIterator>::IntoIter {
        self.into_iter()
    }

    #[inline]
    pub fn iter_mut(&mut self) -> <&mut Sensors as IntoIterator>::IntoIter {
        self.into_iter()
    }
}

impl IntoIterator for Sensors {
    type Item = Sensor;

    type IntoIter = Map<btree_map::IntoIter<u8, Sensor>, fn((u8, Sensor)) -> Sensor>;

    fn into_iter(self) -> Self::IntoIter {
        self.sensors.into_iter().map(|(_, v)| v)
    }
}

impl<'a> IntoIterator for &'a Sensors {
    type Item = &'a Sensor;

    type IntoIter = Map<btree_map::Iter<'a, u8, Sensor>, fn((&u8, &'a Sensor)) -> &'a Sensor>;

    fn into_iter(self) -> Self::IntoIter {
        self.sensors.iter().map(|(_, v)| v)
    }
}

impl<'a> IntoIterator for &'a mut Sensors {
    type Item = &'a mut Sensor;

    type IntoIter =
        Map<btree_map::IterMut<'a, u8, Sensor>, fn((&u8, &'a mut Sensor)) -> &'a mut Sensor>;

    fn into_iter(self) -> Self::IntoIter {
        self.sensors.iter_mut().map(|(_, v)| v)
    }
}
