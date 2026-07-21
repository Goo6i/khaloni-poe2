use std::{process::Command, thread::sleep, time::Duration};

use evdev::{uinput::VirtualDeviceBuilder, AttributeSet, EventType, InputEvent, Key};

fn key(dev: &mut evdev::uinput::VirtualDevice, k: Key, down: bool) {
    dev.emit(&[InputEvent::new(EventType::KEY, k.code(), down as i32)])
        .expect("emit");
    sleep(Duration::from_millis(25));
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut keys = AttributeSet::<Key>::new();
    keys.insert(Key::KEY_LEFTCTRL);
    keys.insert(Key::KEY_C);
    let mut dev = VirtualDeviceBuilder::new()?
        .name("poe2-lens-spike-kbd")
        .with_keys(&keys)?
        .build()?;

    // Give the compositor a moment to register the new virtual keyboard.
    sleep(Duration::from_millis(700));
    eprintln!("focus the game and hover an item; injecting Ctrl+C in 4s");
    sleep(Duration::from_secs(4));

    key(&mut dev, Key::KEY_LEFTCTRL, true);
    key(&mut dev, Key::KEY_C, true);
    key(&mut dev, Key::KEY_C, false);
    key(&mut dev, Key::KEY_LEFTCTRL, false);
    sleep(Duration::from_millis(300));

    let out = Command::new("wl-paste").arg("-n").output()?;
    let text = String::from_utf8_lossy(&out.stdout);
    println!("--- clipboard ({} bytes) ---\n{}", text.len(), text);
    Ok(())
}
