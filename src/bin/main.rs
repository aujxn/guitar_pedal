use std::io;
use structopt::StructOpt;

extern crate guitar_pedal;

#[derive(Debug, StructOpt)]
struct Opt {
    #[structopt(short, long, default_value = "80")]
    bpm: usize,
}

fn main() {
    let opt = Opt::from_args();

    let (playback_manager, loop_manager, interface) = guitar_pedal::init(opt.bpm);
    loop_manager.run();
    interface.run();

    guitar_pedal::activate_client(playback_manager);

    println!("Press enter/return to quit...");
    let mut user_input = String::new();
    io::stdin().read_line(&mut user_input).ok();
}
