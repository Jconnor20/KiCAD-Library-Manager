pub const DEFAULT: &[&str] = &[
    "Resistors",
    "Capacitors",
    "Inductors",
    "Diodes",
    "Transistors",
    "ICs",
    "Connectors",
    "Electromechanical",
    "Crystals_Oscillators",
    "Power",
    "Sensors",
    "Optoelectronics",
    "RF",
    "Misc",
];

/// Word prefixes → category. First match wins, so more specific words come first.
const KEYWORDS: &[(&str, &str)] = &[
    ("resistor", "Resistors"),
    ("potentiometer", "Resistors"),
    ("thermistor", "Resistors"),
    ("capacitor", "Capacitors"),
    ("inductor", "Inductors"),
    ("ferrite", "Inductors"),
    ("choke", "Inductors"),
    ("led", "Optoelectronics"),
    ("optocoupler", "Optoelectronics"),
    ("photo", "Optoelectronics"),
    ("display", "Optoelectronics"),
    ("diode", "Diodes"),
    ("rectifier", "Diodes"),
    ("zener", "Diodes"),
    ("tvs", "Diodes"),
    ("transistor", "Transistors"),
    ("mosfet", "Transistors"),
    ("bjt", "Transistors"),
    ("igbt", "Transistors"),
    ("connector", "Connectors"),
    ("header", "Connectors"),
    ("socket", "Connectors"),
    ("terminal", "Connectors"),
    ("usb", "Connectors"),
    ("crystal", "Crystals_Oscillators"),
    ("oscillator", "Crystals_Oscillators"),
    ("resonator", "Crystals_Oscillators"),
    ("regulator", "Power"),
    ("ldo", "Power"),
    ("converter", "Power"),
    ("pmic", "Power"),
    ("battery", "Power"),
    ("sensor", "Sensors"),
    ("accelerometer", "Sensors"),
    ("gyro", "Sensors"),
    ("magnetometer", "Sensors"),
    ("imu", "Sensors"),
    ("antenna", "RF"),
    ("rf", "RF"),
    ("switch", "Electromechanical"),
    ("relay", "Electromechanical"),
    ("button", "Electromechanical"),
    ("fuse", "Electromechanical"),
    ("buzzer", "Electromechanical"),
    ("microcontroller", "ICs"),
    ("mcu", "ICs"),
    ("amplifier", "ICs"),
    ("opamp", "ICs"),
    ("logic", "ICs"),
    ("memory", "ICs"),
    ("flash", "ICs"),
    ("driver", "ICs"),
    ("interface", "ICs"),
    ("ic", "ICs"),
];

pub fn guess(text: &str) -> &'static str {
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    KEYWORDS.iter().find(|(kw, _)| words.iter().any(|w| w.starts_with(kw))).map_or("Misc", |(_, cat)| cat)
}

#[cfg(test)]
mod tests {
    #[test]
    fn guesses() {
        assert_eq!(super::guess("Resistor Chip"), "Resistors");
        assert_eq!(super::guess("3-Axis Magnetometers"), "Sensors");
        assert_eq!(super::guess("LED Indication - Discrete"), "Optoelectronics");
        assert_eq!(super::guess("scaled widget"), "Misc");
    }
}
