
#![no_std]
#![no_main]

mod limine;

use core::arch::asm;
use core::panic::PanicInfo;

const COM1: u16 = 0x3F8;

// Serial port output
unsafe fn outb(port: u16, value: u8) {
    unsafe {
        asm!(
            "out dx, al",
            in("dx") port,
            in("al") value,
            options(nomem, nostack, preserves_flags)
        );
    }
}

// Serial port input
unsafe fn inb(port: u16) -> u8 {
    let value: u8;

    unsafe {
        asm!(
            "in al, dx",
            in("dx") port,
            out("al") value,
            options(nomem, nostack, preserves_flags)
        );
    }

    value
}

// Initialize COM1
fn serial_init() {
    unsafe {
        outb(COM1 + 1, 0x00);
        outb(COM1 + 3, 0x80);
        outb(COM1, 0x03);
        outb(COM1 + 1, 0x00);
        outb(COM1 + 3, 0x03);
        outb(COM1 + 2, 0xC7);
        outb(COM1 + 4, 0x0B);
    }
}

// Write one byte
fn serial_write_byte(byte: u8) {
    unsafe {
        while (inb(COM1 + 5) & 0x20) == 0 {
            core::hint::spin_loop();
        }

        outb(COM1, byte);
    }
}

// Write a string
fn serial_write(message: &str) {
    for byte in message.bytes() {
        serial_write_byte(byte);
    }
}

// Write an unsigned decimal integer
fn serial_write_u64(mut value: u64) {
    let mut buffer = [0u8; 20];
    let mut index = buffer.len();

    if value == 0 {
        serial_write("0");
        return;
    }

    while value > 0 {
        index -= 1;
        buffer[index] = b'0' + (value % 10) as u8;
        value /= 10;
    }

    for byte in &buffer[index..] {
        serial_write_byte(*byte);
    }
}

// Kernel entry point
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    serial_init();

    serial_write("\r\n[BOOT] Asternel kernel started.\r\n");

    // Read Limine memory map response
    match limine::memmap_response_info() {
        Some((revision, entry_count, _entries)) => {
            serial_write("[MEM] Response received.\r\n");

            serial_write("[MEM] Response revision: ");
            serial_write_u64(revision);

            serial_write("\r\n[MEM] Entry count: ");
            serial_write_u64(entry_count);
            serial_write("\r\n");

            // Read individual memory map entries
            let mut index = 0;

            while index < entry_count {
                match limine::memmap_entry_at(index) {
                    Some((base, length, entry_type)) => {
                        serial_write("[MEM] Entry ");
                        serial_write_u64(index);

                        serial_write(" | Base: ");
                        serial_write_u64(base);

                        serial_write(" | Length: ");
                        serial_write_u64(length);

                        serial_write(" | Type: ");
                        serial_write_u64(entry_type);

                        serial_write("\r\n");
                    }

                    None => {
                        serial_write("[MEM] Invalid entry at index ");
                        serial_write_u64(index);
                        serial_write("\r\n");
                        break;
                    }
                }

                index += 1;
            }
        }

        None => {
            serial_write("[MEM] Invalid or missing response.\r\n");
        }
    }

    serial_write("\r\n");
    serial_write("================================\r\n");
    serial_write("      ASTERNEL KERNEL v0.1\r\n");
    serial_write("      x86_64 architecture\r\n");
    serial_write("      Serial console: COM1\r\n");
    serial_write("================================\r\n");
    serial_write("Kernel entry successful.\r\n");

    loop {
        core::hint::spin_loop();
    }
}

// Panic handler
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    serial_write("\r\nKERNEL PANIC\r\n");

    loop {
        core::hint::spin_loop();
    }
}
