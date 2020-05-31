//! Creates a jack midi input and output ports. The application prints
//! out all values sent to it through the input port. It also sends a
//! Note On and Off event, once every cycle, on the output port.
use std::convert::From;
use std::io;
use std::sync::mpsc::sync_channel;

const MAX_MIDI: usize = 3;

//a fixed size container to copy data out of real-time thread
#[derive(Copy, Clone)]
pub struct MidiEvent {
    pub len: usize,
    pub data: [u8; MAX_MIDI],
    pub time: jack::Frames,
}

impl From<jack::RawMidi<'_>> for MidiEvent {
    fn from(midi: jack::RawMidi<'_>) -> Self {
        let len = std::cmp::min(MAX_MIDI, midi.bytes.len());
        let mut data = [0; MAX_MIDI];
        data[..len].copy_from_slice(&midi.bytes[..len]);
        MidiEvent {
            len,
            data,
            time: midi.time,
        }
    }
}

impl std::fmt::Debug for MidiEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "Midi {{ time: {}, len: {}, data: {:?} }}",
            self.time,
            self.len,
            &self.data[..self.len]
        )
    }
}

pub fn listen_for_midi() -> std::sync::mpsc::Receiver<MidiEvent> {
    // open client
    let (client, _status) =
        jack::Client::new("rust_jack_show_midi", jack::ClientOptions::NO_START_SERVER).unwrap();

    //create a sync channel to send back copies of midi messages we get
    let (sender, receiver) = sync_channel(64);

    std::thread::spawn(move || {
        let midi_controller = client
            .register_port("midi_loop_controller", jack::MidiIn::default())
            .unwrap();

        let cback = move |_: &jack::Client, ps: &jack::ProcessScope| -> jack::Control {
            let iter = midi_controller.iter(ps);
            for e in iter {
                let c: MidiEvent = e.into();
                let _ = sender.try_send(c);
            }
            jack::Control::Continue
        };

        // activate
        let _active_client = client
            .activate_async((), jack::ClosureProcessHandler::new(cback))
            .unwrap();

        loop {
            std::thread::park();
        }
    });

    /*
    //spawn a non-real-time thread that prints out the midi messages we get
    std::thread::spawn(move || {
        while let Ok(m) = receiver.recv() {
            if m.data[0] != 248u8 && m.data[0] != 254u8 {
                println!("{:?}", m);
            }
        }
    });
    */

    receiver
}
