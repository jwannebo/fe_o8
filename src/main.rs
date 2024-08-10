use crossterm::{
    cursor, execute, queue,
    style::{Color, Print, PrintStyledContent, ResetColor, StyledContent, Stylize},
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
    QueueableCommand,
};
use evdev::Key;
use rand::random;
use rodio::{
    source::{SineWave, Source},
    OutputStream, Sink,
};
use std::{
    env,
    error::Error,
    fs::File,
    io::{prelude::*, stdout, Stdout},
    path::Path,
    result::Result,
    thread::sleep,
    time::Instant,
};

static VAR_AND_DISPLAY_REFRESH_SIZE: u16 = 352;
static MEMORY_SIZE: u16 = 0x1000;
static ADDR_START_PROGRAM: u16 = 0x200;
static ADDR_PROGRAM_END: u16 = MEMORY_SIZE - VAR_AND_DISPLAY_REFRESH_SIZE;

struct Chip8 {
    memory: [u8; 0x1000],
    display: [u64; 32],
    pc: u16,
    stack: Vec<u16>,
    delay: u8,
    sound: u8,
    v: [u8; 16],
    i: u16,
}

#[derive(Debug)]
struct Opcode {
    n0: u8,
    n1: u8,
    n2: u8,
    n3: u8,
    a: u16,
    v: u8,
}
impl Opcode {
    fn from_slice(slice: &[u8]) -> Opcode {
        assert!(slice.len() > 2);
        Opcode {
            n0: (slice[0] & 0xF0) >> 4,
            n1: slice[0] & 0x0F,
            n2: (slice[1] & 0xF0) >> 4,
            n3: slice[1] & 0x0F,
            a: (slice[0] as u16 & 0x0F) << 8 | slice[1] as u16,
            v: slice[1],
        }
    }
}

fn style_number(number: u8, keys: [bool; 16]) -> StyledContent<String> {
    let color = if keys[number as usize] {
        Color::Black
    } else {
        Color::White
    };
    let background = if keys[number as usize] {
        Color::White
    } else {
        Color::Black
    };
    format!("{:x}", number).with(color).on(background)
}

fn color_from_index(index: usize) -> Color {
    match index {
        0 => Color::AnsiValue(21),
        1 => Color::AnsiValue(20),
        2 => Color::AnsiValue(19),
        3 => Color::AnsiValue(18),
        _ => Color::AnsiValue(17),
    }
}

fn print_memory<'std>(
    c8: &Chip8,
    stdout: &'std mut Stdout,
) -> Result<&'std mut Stdout, Box<dyn Error>> {
    for i in (0..MEMORY_SIZE).step_by(32) {
        let rng = i..(i + 32);
        let mut color: Color;
        let mut character = if rng.contains(&c8.pc) {
            'P'
        } else if rng.contains(&c8.i) {
            'i'
        } else {
            '┄'
        };
        if i < ADDR_START_PROGRAM {
            color = Color::Black;
        } else {
            color = Color::Reset;
        }

        for (j, addr) in c8.stack.iter().rev().enumerate() {
            if rng.contains(addr) {
                character = 's';
                color = color_from_index(j);
            }
        }
        stdout.queue(PrintStyledContent(format!("{}", character).on(color)))?;
    }
    Ok(stdout)
}

struct DeviceWrapper(evdev::Device);

impl Drop for DeviceWrapper {
    fn drop(&mut self) {
        _ = self.0.ungrab();
        _ = terminal::disable_raw_mode();
        _ = execute!(
            stdout(),
            LeaveAlternateScreen,
            Clear(ClearType::All),
            cursor::RestorePosition,
            cursor::EnableBlinking,
            cursor::Show
        );
    }
}

static REQUIRED_KEYS: [Key; 17] = [
    Key::KEY_ESC,
    Key::KEY_X,
    Key::KEY_1,
    Key::KEY_2,
    Key::KEY_3,
    Key::KEY_Q,
    Key::KEY_W,
    Key::KEY_E,
    Key::KEY_A,
    Key::KEY_S,
    Key::KEY_D,
    Key::KEY_Z,
    Key::KEY_C,
    Key::KEY_4,
    Key::KEY_R,
    Key::KEY_F,
    Key::KEY_V,
];
static FONT_ARR: [u8; 16 * 5] = [
    0xF0, 0x90, 0x90, 0x90, 0xF0, // 0
    0x20, 0x60, 0x20, 0x20, 0x70, // 1
    0xF0, 0x10, 0xF0, 0x80, 0xF0, // 2
    0xF0, 0x10, 0xF0, 0x10, 0xF0, // 3
    0x90, 0x90, 0xF0, 0x10, 0x10, // 4
    0xF0, 0x80, 0xF0, 0x10, 0xF0, // 5
    0xF0, 0x80, 0xF0, 0x90, 0xF0, // 6
    0xF0, 0x10, 0x20, 0x40, 0x40, // 7
    0xF0, 0x90, 0xF0, 0x90, 0xF0, // 8
    0xF0, 0x90, 0xF0, 0x10, 0xF0, // 9
    0xF0, 0x90, 0xF0, 0x90, 0x90, // A
    0xE0, 0x90, 0xE0, 0x90, 0xE0, // B
    0xF0, 0x80, 0x80, 0x80, 0xF0, // C
    0xE0, 0x90, 0x90, 0x90, 0xE0, // D
    0xF0, 0x80, 0xF0, 0x80, 0xF0, // E
    0xF0, 0x80, 0xF0, 0x80, 0x80, // F
];
static FONT_ADDR: [u16; 16] = [
    0x050, // 0
    0x055, // 1
    0x05A, // 2
    0x05F, // 3
    0x064, // 4
    0x069, // 5
    0x06E, // 6
    0x073, // 7
    0x078, // 8
    0x07D, // 9
    0x082, // A
    0x087, // B
    0x08C, // C
    0x091, // D
    0x096, // E
    0x09A, // F
];

fn main() -> Result<(), Box<dyn Error>> {
    // Get keyboard

    let devices = evdev::enumerate().map(|t| t.1).collect::<Vec<_>>();
    let mut device = DeviceWrapper(
        devices
            .into_iter()
            .find(|d| {
                if let Some(supported) = d.supported_keys() {
                    REQUIRED_KEYS.iter().all(|k| supported.contains(*k))
                } else {
                    false
                }
            })
            .expect("Could not find keyboard device supporting required keys"),
    );

    // Setup Display

    let mut stdout = stdout();
    terminal::enable_raw_mode()?;
    device.0.grab()?;
    execute!(
        stdout,
        EnterAlternateScreen,
        Clear(ClearType::All),
        cursor::Hide,
        cursor::DisableBlinking,
        cursor::SavePosition
    )?;

    // Open file

    let args: Vec<String> = env::args().collect();
    let path = Path::new(&args[1]);
    let file = File::open(path)?;

    //Initialize main memory

    let mut chip8 = Chip8 {
        memory: [0; 0x1000],
        display: [0; 32],
        pc: ADDR_START_PROGRAM,
        stack: vec![],
        delay: 0x0,
        sound: 0x0,
        v: [0; 16],
        i: 0x0,
    };

    chip8.memory[0x050..0x0A0].copy_from_slice(&FONT_ARR);

    if let Err(e) = file
        .take((ADDR_PROGRAM_END - ADDR_START_PROGRAM) as u64)
        .read_exact(&mut chip8.memory[ADDR_START_PROGRAM as usize..ADDR_PROGRAM_END as usize])
    {
        if e.kind() != std::io::ErrorKind::UnexpectedEof {
            return Err(Box::new(e));
        }
    }

    //Set up sound

    let (_stream, stream_handle) = OutputStream::try_default()?;
    let sink = Sink::try_new(&stream_handle)?;
    let beep = SineWave::new(440.0).amplify(0.8);
    sink.append(beep);
    sink.pause();

    let mut last_time = Instant::now();
    let mut keys = [false; 16];

    'exit: loop {
        if last_time.elapsed().as_secs_f32() * 60.0 < 1.0 {
            sleep(Instant::now() - last_time);
        } else {
            stdout.queue(cursor::MoveTo(0, 0))?.queue(Print(format!(
                "{:.1}fps {:.4}fpf",
                1.0 / last_time.elapsed().as_secs_f32(),
                last_time.elapsed().as_secs_f32() * 60.0
            )))?;
            last_time = Instant::now();
            let last_keys = keys;
            keys = [false; 16];

            for key in &device
                .0
                .get_key_state()
                .expect("Chosen device should be a keyboard")
            {
                match key {
                    Key::KEY_ESC | Key::KEY_PAUSE => break 'exit,
                    Key::KEY_X => keys[0x0] = true,
                    Key::KEY_1 => keys[0x1] = true,
                    Key::KEY_2 => keys[0x2] = true,
                    Key::KEY_3 => keys[0x3] = true,
                    Key::KEY_Q => keys[0x4] = true,
                    Key::KEY_W => keys[0x5] = true,
                    Key::KEY_E => keys[0x6] = true,
                    Key::KEY_A => keys[0x7] = true,
                    Key::KEY_S => keys[0x8] = true,
                    Key::KEY_D => keys[0x9] = true,
                    Key::KEY_Z => keys[0xA] = true,
                    Key::KEY_C => keys[0xB] = true,
                    Key::KEY_4 => keys[0xC] = true,
                    Key::KEY_R => keys[0xD] = true,
                    Key::KEY_F => keys[0xE] = true,
                    Key::KEY_V => keys[0xF] = true,
                    _ => (),
                }
            }

            queue!(
                stdout,
                cursor::MoveTo(70 + 64, 5),
                PrintStyledContent(style_number(0x1, keys)),
                PrintStyledContent(style_number(0x2, keys)),
                PrintStyledContent(style_number(0x3, keys)),
                PrintStyledContent(style_number(0xC, keys)),
                cursor::MoveTo(70 + 64, 6),
                PrintStyledContent(style_number(0x4, keys)),
                PrintStyledContent(style_number(0x5, keys)),
                PrintStyledContent(style_number(0x6, keys)),
                PrintStyledContent(style_number(0xD, keys)),
                cursor::MoveTo(70 + 64, 7),
                PrintStyledContent(style_number(0x7, keys)),
                PrintStyledContent(style_number(0x8, keys)),
                PrintStyledContent(style_number(0x9, keys)),
                PrintStyledContent(style_number(0xE, keys)),
                cursor::MoveTo(70 + 64, 8),
                PrintStyledContent(style_number(0xA, keys)),
                PrintStyledContent(style_number(0x0, keys)),
                PrintStyledContent(style_number(0xB, keys)),
                PrintStyledContent(style_number(0xF, keys)),
                ResetColor
            )?;

            if chip8.delay > 0 {
                chip8.delay -= 1;
            };
            if chip8.sound > 0 {
                if sink.is_paused() {
                    sink.play();
                }
                chip8.sound -= 1;
            } else if !sink.is_paused() {
                sink.pause();
            }
            stdout
                .queue(cursor::MoveTo(0, 2))?
                .queue(Print(format!("╔{:═<128}╗", "")))?;

            for line in chip8.display {
                let output: String = format!("{:064b}", line)
                    .chars()
                    .map(|c| match c {
                        '1' => "██",
                        '0' => "░░",
                        _ => "  ",
                    })
                    .collect();
                stdout
                    .queue(cursor::MoveToNextLine(1))?
                    .queue(Print::<String>(format!("║{}║", output)))?;
            }
            stdout
                .queue(cursor::MoveToNextLine(1))?
                .queue(Print(format!("╠{:═<128}╣", "")))?;

            stdout.queue(cursor::MoveToNextLine(1))?.queue(Print("╙"))?;
            print_memory(&chip8, &mut stdout)?
                .queue(Print("╜"))?
                .flush()?;

            for _ in 0..12 {
                // Fetch
                let op = Opcode::from_slice(&chip8.memory[chip8.pc as usize..]);

                chip8.pc += 2;
                // Decode and Execute
                match op {
                    Opcode {
                        n0: 0x0,
                        n1: 0x0,
                        n2: 0xE,
                        n3: 0x0,
                        a: _,
                        v: _,
                    } => chip8.display = [0; 32], // CLR
                    Opcode {
                        n0: 0x0,
                        n1: 0x0,
                        n2: 0xE,
                        n3: 0xE,
                        a: _,
                        v: _,
                    } => chip8.pc = chip8.stack.pop().unwrap(), // RTN
                    Opcode {
                        n0: 0x1,
                        n1: _,
                        n2: _,
                        n3: _,
                        a: nnn,
                        v: _,
                    } => chip8.pc = nnn, // JMP
                    Opcode {
                        n0: 0x2,
                        n1: _,
                        n2: _,
                        n3: _,
                        a: nnn,
                        v: _,
                    } => {
                        chip8.stack.push(chip8.pc);
                        chip8.pc = nnn;
                    } // CAL
                    Opcode {
                        n0: 0x3,
                        n1: x,
                        n2: _,
                        n3: _,
                        a: _,
                        v: nn,
                    } => {
                        let x = x as usize;
                        if chip8.v[x] == nn {
                            chip8.pc += 2
                        }
                    } // SEQ
                    Opcode {
                        n0: 0x4,
                        n1: x,
                        n2: _,
                        n3: _,
                        a: _,
                        v: nn,
                    } => {
                        let x = x as usize;
                        if chip8.v[x] != nn {
                            chip8.pc += 2
                        }
                    } // SNE
                    Opcode {
                        n0: 0x5,
                        n1: x,
                        n2: y,
                        n3: 0x0,
                        a: _,
                        v: _,
                    } => {
                        let x = x as usize;
                        let y = y as usize;
                        if chip8.v[x] == chip8.v[y] {
                            chip8.pc += 2
                        }
                    } // SER
                    Opcode {
                        n0: 0x6,
                        n1: x,
                        n2: _,
                        n3: _,
                        a: _,
                        v: nn,
                    } => chip8.v[x as usize] = nn, // CAN
                    Opcode {
                        n0: 0x7,
                        n1: x,
                        n2: _,
                        n3: _,
                        a: _,
                        v: nn,
                    } => {
                        let x = x as usize;
                        let (value, ..) = chip8.v[x].overflowing_add(nn);
                        chip8.v[x] = value;
                    } // CAD
                    Opcode {
                        n0: 0x8,
                        n1: x,
                        n2: y,
                        n3: 0x0,
                        a: _,
                        v: _,
                    } => chip8.v[x as usize] = chip8.v[y as usize], // ASN
                    Opcode {
                        n0: 0x8,
                        n1: x,
                        n2: y,
                        n3: 0x1,
                        a: _,
                        v: _,
                    } => {
                        chip8.v[x as usize] |= chip8.v[y as usize];
                        chip8.v[0xF] = 0
                    } // ORR
                    Opcode {
                        n0: 0x8,
                        n1: x,
                        n2: y,
                        n3: 0x2,
                        a: _,
                        v: _,
                    } => {
                        chip8.v[x as usize] &= chip8.v[y as usize];
                        chip8.v[0xF] = 0
                    } // AND
                    Opcode {
                        n0: 0x8,
                        n1: x,
                        n2: y,
                        n3: 0x3,
                        a: _,
                        v: _,
                    } => {
                        chip8.v[x as usize] ^= chip8.v[y as usize];
                        chip8.v[0xF] = 0
                    } // XOR
                    Opcode {
                        n0: 0x8,
                        n1: x,
                        n2: y,
                        n3: 0x4,
                        a: _,
                        v: _,
                    } => {
                        let x = x as usize;
                        let y = y as usize;
                        let (value, carry) = chip8.v[x].overflowing_add(chip8.v[y]);
                        chip8.v[x] = value;
                        chip8.v[0xF] = carry as u8;
                    } // ADD
                    Opcode {
                        n0: 0x8,
                        n1: x,
                        n2: y,
                        n3: 0x5,
                        a: _,
                        v: _,
                    } => {
                        let x = x as usize;
                        let y = y as usize;
                        let (value, carry) = chip8.v[x].overflowing_sub(chip8.v[y]);
                        chip8.v[x] = value;
                        chip8.v[0xF] = !carry as u8;
                    } // SXY
                    Opcode {
                        n0: 0x8,
                        n1: x,
                        n2: y,
                        n3: 0x6,
                        a: _,
                        v: _,
                    } => {
                        let x = x as usize;
                        let y = y as usize;
                        let carry = chip8.v[y] & 0x1;
                        let value = chip8.v[y] >> 1;
                        chip8.v[x] = value;
                        chip8.v[0xF] = carry;
                    } // RSH
                    Opcode {
                        n0: 0x8,
                        n1: x,
                        n2: y,
                        n3: 0x7,
                        a: _,
                        v: _,
                    } => {
                        let x = x as usize;
                        let y = y as usize;
                        let (value, carry) = chip8.v[y].overflowing_sub(chip8.v[x]);
                        chip8.v[x] = value;
                        chip8.v[0xF] = !carry as u8;
                    } // SYX
                    Opcode {
                        n0: 0x8,
                        n1: x,
                        n2: y,
                        n3: 0xE,
                        a: _,
                        v: _,
                    } => {
                        let x = x as usize;
                        let y = y as usize;
                        let carry = (chip8.v[y] & 0b1000_0000) >> 7;
                        let value = chip8.v[y] << 1;
                        chip8.v[x] = value;
                        chip8.v[0xF] = carry;
                    } // LSH
                    Opcode {
                        n0: 0x9,
                        n1: x,
                        n2: y,
                        n3: 0x0,
                        a: _,
                        v: _,
                    } => {
                        let x = x as usize;
                        let y = y as usize;
                        if chip8.v[x] != chip8.v[y] {
                            chip8.pc += 2
                        }
                    } // SNR
                    Opcode {
                        n0: 0xA,
                        n1: _,
                        n2: _,
                        n3: _,
                        a: nnn,
                        v: _,
                    } => chip8.i = nnn, // CAI
                    Opcode {
                        n0: 0xB,
                        n1: _,
                        n2: _,
                        n3: _,
                        a: nnn,
                        v: _,
                    } => chip8.pc = nnn + chip8.v[0] as u16, // J0N
                    Opcode {
                        n0: 0xC,
                        n1: x,
                        n2: _,
                        n3: _,
                        a: _,
                        v: nn,
                    } => chip8.v[x as usize] = random::<u8>() & nn, // RND
                    Opcode {
                        n0: 0xD,
                        n1: x,
                        n2: y,
                        n3: n,
                        a: _,
                        v: _,
                    } => {
                        let x = x as usize;
                        let y = y as usize;
                        let coord_x = chip8.v[x] % 64;
                        let mut coord_y = chip8.v[y] as usize % 32;
                        chip8.v[0xF] = 0;
                        let mut i = chip8.i as usize;
                        let imax = i + n as u16 as usize;
                        while coord_y < 32 && i < imax {
                            // Operate on a u128, with 32 bits of padding to avoid overlfow

                            // First, put the sprite at coord 0 (bit 32) by lshifting it 32 (pad) + 64 (screen width) - 8 (byte width)
                            // 00000000000000000000000000000000|SSSSSSSS00000000000000000000000000000000000000000000000000000000|00000000000000000000000000000000
                            let sprite = (chip8.memory[i] as u128) << (32 + 64 - 8);

                            // Then rshift it to it's proper x position
                            // 00000000000000000000000000000000|000SSSSSSSS00000000000000000000000000000000000000000000000000000|00000000000000000000000000000000
                            //                                 |x-|
                            let sprite = sprite >> coord_x;

                            // Then do an overflow aware rshift of 32 to squish the display 64 into the lower 64
                            //0000000000000000000000000000000000000000000000000000000000000000|000SSSSSSSS00000000000000000000000000000000000000000000000000000
                            let mask = sprite.rotate_right(32);

                            //Then grab only the 64 bits we care about
                            //000SSSSSSSS00000000000000000000000000000000000000000000000000000
                            let mask = (mask & 0xFFFF_FFFF_FFFF_FFFF) as u64;

                            chip8.v[0xF] = if mask & chip8.display[coord_y] > 0 {
                                0x1
                            } else {
                                0x0
                            };
                            chip8.display[coord_y] ^= mask;

                            coord_y += 1;
                            i += 1;
                        }
                    } // DRW
                    Opcode {
                        n0: 0xE,
                        n1: x,
                        n2: 0x9,
                        n3: 0xE,
                        a: _,
                        v: _,
                    } => {
                        if keys[chip8.v[x as usize] as usize & 0x0F] {
                            chip8.pc += 2;
                        }
                    } // KYP
                    Opcode {
                        n0: 0xE,
                        n1: x,
                        n2: 0xA,
                        n3: 0x1,
                        a: _,
                        v: _,
                    } => {
                        if !keys[chip8.v[x as usize] as usize & 0x0F] {
                            chip8.pc += 2;
                        }
                    } // KYR
                    Opcode {
                        n0: 0xF,
                        n1: x,
                        n2: 0x0,
                        n3: 0x7,
                        a: _,
                        v: _,
                    } => chip8.v[x as usize] = chip8.delay, // DLX
                    Opcode {
                        n0: 0xF,
                        n1: x,
                        n2: 0x0,
                        n3: 0xA,
                        a: _,
                        v: _,
                    } => {
                        chip8.pc -= 2;
                        'char: for k in 0x0..=0xF {
                            if last_keys[k] && (last_keys[k] ^ keys[k]) {
                                chip8.v[x as usize] = k as u8;
                                chip8.pc += 2;
                                break 'char;
                            }
                        }
                    } // BKY
                    Opcode {
                        n0: 0xF,
                        n1: x,
                        n2: 0x1,
                        n3: 0x5,
                        a: _,
                        v: _,
                    } => chip8.delay = chip8.v[x as usize], // DYS
                    Opcode {
                        n0: 0xF,
                        n1: x,
                        n2: 0x1,
                        n3: 0x8,
                        a: _,
                        v: _,
                    } => chip8.sound = chip8.v[x as usize], // SND
                    Opcode {
                        n0: 0xF,
                        n1: x,
                        n2: 0x1,
                        n3: 0xE,
                        a: _,
                        v: _,
                    } => {
                        let x = x as usize;
                        let value = chip8.i + chip8.v[x] as u16;
                        chip8.v[0xF] = (value & 0xF000 > 0) as u8;
                        chip8.i = value;
                    } // ADI
                    Opcode {
                        n0: 0xF,
                        n1: x,
                        n2: 0x2,
                        n3: 0x9,
                        a: _,
                        v: _,
                    } => chip8.i = FONT_ADDR[chip8.v[x as usize] as usize & 0x0F], // RCH
                    Opcode {
                        n0: 0xF,
                        n1: x,
                        n2: 0x3,
                        n3: 0x3,
                        a: _,
                        v: _,
                    } => {
                        let x = x as usize;
                        let i = chip8.i as usize;
                        chip8.memory[i + 0] = chip8.v[x] / 100;
                        chip8.memory[i + 1] = (chip8.v[x] % 100) / 10;
                        chip8.memory[i + 2] = chip8.v[x] % 10;
                    } // BCD
                    Opcode {
                        n0: 0xF,
                        n1: x,
                        n2: 0x5,
                        n3: 0x5,
                        a: _,
                        v: _,
                    } => {
                        let x = x as usize;
                        let i = chip8.i as usize;
                        chip8.memory[i..=i + x].copy_from_slice(&chip8.v[0..=x])
                    } // RST
                    Opcode {
                        n0: 0xF,
                        n1: x,
                        n2: 0x6,
                        n3: 0x5,
                        a: _,
                        v: _,
                    } => {
                        let x = x as usize;
                        let i = chip8.i as usize;
                        chip8.v[0..=x].copy_from_slice(&chip8.memory[i..=i + x])
                    } // RLD

                    _ => panic!("Unknown operand! {0:?}", op),
                };
            }
        }
    }

    Ok(())
}
