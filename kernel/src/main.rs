
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

// Detect CPU physical address width
fn physical_address_width() -> Option<u8> {
    use core::arch::x86_64::__cpuid;

    let max_leaf = unsafe {
        __cpuid(0x80000000).eax
    };

    if max_leaf < 0x80000008 {
        return None;
    }

    let result = unsafe {
        __cpuid(0x80000008)
    };

    let width = (result.eax & 0xFF) as u8;

    if !(36..=52).contains(&width) {
        return None;
    }

    Some(width)
}

// Build the physical address mask from the detected CPU width.
fn physical_address_mask(width: u8) -> Option<u64> {
    if !(36..=52).contains(&width) {
        return None;
    }

    Some(((1u64 << width) - 1) & 0x000F_FFFF_FFFF_F000)
}



// Kernel entry point
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    serial_init();

    serial_write("\r\n[BOOT] Asternel kernel started.\r\n");

    // Executable-file response
   serial_write("[KERNEL] Checking executable-address response...\r\n");

match limine::executable_address_info() {
    Some((physical_base, virtual_base)) => {
        serial_write("[KERNEL] Executable-address response received.\r\n");

        serial_write("[KERNEL] Physical base: ");
        serial_write_u64(physical_base);
        serial_write("\r\n");

        serial_write("[KERNEL] Virtual base: ");
        serial_write_u64(virtual_base);
        serial_write("\r\n");
    }

    None => {
        serial_write("[KERNEL] Executable-address response unavailable.\r\n");
    }
}

serial_write("[KERNEL] Validating kernel image reservation...\n");

match limine::kernel_image_reservation() {
    Ok((start, end)) => {
        serial_write("[KERNEL] Kernel image reservation validated.\n");
        serial_write("[KERNEL] Reservation start: ");
        serial_write_u64(start);
        serial_write("\n");

        serial_write("[KERNEL] Reservation end: ");
        serial_write_u64(end);
        serial_write("\n");
    }

    Err(error) => {
        serial_write("[KERNEL] Kernel image reservation validation FAILED.\n");

        match error {
            limine::KernelReservationError::MissingExecutableAddress =>
                serial_write("[KERNEL] Missing executable address.\n"),
            limine::KernelReservationError::InvalidAlignment =>
                serial_write("[KERNEL] Invalid address alignment.\n"),
            limine::KernelReservationError::AddressOverflow =>
                serial_write("[KERNEL] Address overflow.\n"),
            limine::KernelReservationError::MissingMemoryMap =>
                serial_write("[KERNEL] Missing memory map.\n"),
            limine::KernelReservationError::InvalidMemoryMap =>
                serial_write("[KERNEL] Invalid memory map.\n"),
            limine::KernelReservationError::KernelRangeNotFound =>
                serial_write("[KERNEL] Kernel range not found.\n"),
            limine::KernelReservationError::KernelRangeNotExecutable =>
                serial_write("[KERNEL] Kernel range is not executable memory.\n"),

            limine::KernelReservationError::InvalidLinkerBounds =>
    serial_write("[KERNEL] Invalid linker image bounds.\n"),

limine::KernelReservationError::VirtualBaseMismatch =>
    serial_write("[KERNEL] Linker and executable virtual bases differ.\n"),
        }
    }
}

    

    // HHDM response
    serial_write("[HHDM] Checking Higher Half Direct Map...\r\n");

    match limine::hhdm_response_info() {
        Some((revision, offset)) => {
            serial_write("[HHDM] Response received.\r\n");

            serial_write("[HHDM] Response revision: ");
            serial_write_u64(revision);
            serial_write("\r\n");

            serial_write("[HHDM] Physical memory offset: ");
            serial_write_u64(offset);
            serial_write("\r\n");

            serial_write("[HHDM] Basic validation PASSED.\r\n");
        }

        None => {
            serial_write("[HHDM] Response missing or invalid.\r\n");

            loop {
                core::hint::spin_loop();
            }
        }
    }

    // Physical-to-virtual conversion test
    serial_write("[HHDM] Testing physical-to-virtual conversion...\r\n");

    match limine::physical_to_virtual(0x0010_0000) {
        Some(address) => {
            serial_write("[HHDM] Physical address: 1048576\r\n");

            serial_write("[HHDM] Converted virtual address: ");
            serial_write_u64(address as u64);
            serial_write("\r\n");

            serial_write("[HHDM] Conversion PASSED.\r\n");
        }

        None => {
            serial_write("[HHDM] Conversion FAILED.\r\n");

            loop {
                core::hint::spin_loop();
            }
        }
    }

    // Overflow rejection test
    if limine::physical_to_virtual(u64::MAX).is_none() {
        serial_write("[HHDM] Overflow rejection PASSED.\r\n");
    } else {
        serial_write("[HHDM] Overflow rejection FAILED.\r\n");

        loop {
            core::hint::spin_loop();
        }
    }

    // Memory-map boundary rejection test
    if limine::physical_to_virtual(0xFFFFFFFFFFFFF000).is_none() {
        serial_write("[HHDM] Out-of-range rejection PASSED.\r\n");
    } else {
        serial_write("[HHDM] Out-of-range rejection FAILED.\r\n");

        loop {
            core::hint::spin_loop();
        }
    }

    // Memory-map validation
    serial_write("[MEM] Starting memory-map validation...\r\n");

    match limine::validate_memmap() {
        Ok(unknown_count) => {
            serial_write("[MEM] Validation PASSED.\r\n");

            serial_write("[MEM] Unknown entry types: ");
            serial_write_u64(unknown_count);
            serial_write("\r\n");
        }

        Err(error) => {
            serial_write("[MEM] Validation FAILED. Error code: ");

            let code = match error {
                limine::MemmapValidationError::MissingResponse => 1,
                limine::MemmapValidationError::TooManyEntries => 2,
                limine::MemmapValidationError::InvalidEntry(_) => 3,
                limine::MemmapValidationError::UnsortedOrOverlapping(_) => 4,
            };

            serial_write_u64(code);
            serial_write("\r\n");

            loop {
                core::hint::spin_loop();
            }
        }
    }

    // Read-only paging mode discovery
    serial_write("[MM] Checking active paging mode...\r\n");

    let cr4: u64;

    unsafe {
        asm!(
            "mov {}, cr4",
            out(reg) cr4,
            options(nomem, nostack, preserves_flags)
        );
    }

    serial_write("[MM] CR4 value: ");
    serial_write_u64(cr4);
    serial_write("\r\n");

    if (cr4 & (1 << 12)) == 0 {
        serial_write("[MM] Five-level paging (LA57): DISABLED\r\n");
        serial_write("[MM] Active paging mode: FOUR-LEVEL\r\n");
    } else {
        serial_write("[MM] Five-level paging (LA57): ENABLED\r\n");
        serial_write("[MM] Active paging mode: FIVE-LEVEL\r\n");
    }


    // CPU physical address width discovery
    serial_write("[MM] Checking CPU physical address width...\r\n");

    match physical_address_width() {
        Some(width) => {
            serial_write("[MM] Physical address width: ");
            serial_write_u64(width as u64);
            serial_write(" bits\r\n");
        }

        None => {
            serial_write(
                "[MM] Physical address width unavailable or invalid.\r\n"
            );
        }
    }

    
    
    // Read-only active page-table root discovery
    serial_write("[MM] Reading active CR3 register...\r\n");

    let cr3: u64;

    unsafe {
        asm!(
            "mov {}, cr3",
            out(reg) cr3,
            options(nomem, nostack, preserves_flags)
        );
    }

    let root_page = cr3 & !(limine::PAGE_SIZE - 1);

    serial_write("[MM] CR3 value: ");
    serial_write_u64(cr3);
    serial_write("\r\n");

    serial_write("[MM] Page-table root physical address: ");
    serial_write_u64(root_page);
    serial_write("\r\n");

    match root_page.checked_add(limine::PAGE_SIZE) {
        Some(root_end) => {
            let mut found = false;
            let mut index = 0;

            if let Some((_, entry_count, _)) =
                limine::memmap_response_info()
            {
                while index < entry_count {
                    if let Some((base, length, entry_type)) =
                        limine::memmap_entry_at(index)
                    {
                        if let Some(entry_end) = base.checked_add(length) {
                            if root_page >= base && root_end <= entry_end {
                                serial_write(
                                    "[MM] Page-table root memory type: "
                                );
                                serial_write_u64(entry_type);
                                serial_write("\r\n");

                                found = true;
                                break;
                            }
                        }
                    }

                    index += 1;
                }
            }

            if found {
                serial_write(
                    "[MM] Page-table root classification completed.\r\n"
                );
            } else {
                serial_write(
                    "[MM] Page-table root is not covered by a memory-map entry.\r\n"
                );
            }
        }

        None => {
            serial_write("[MM] Page-table root range overflow.\r\n");
        }
    }


    // Read-only PML4 inspection
    serial_write("[MM] Inspecting PML4 root entries...\r\n");

   
    match limine::physical_to_virtual(root_page) {
        Some(root_virtual) => {
            let pml4 = root_virtual as *const u64;
            let mut present_count = 0u64;
            let mut index = 0usize;

            while index < 512 {
                let entry = unsafe {
                    core::ptr::read_volatile(pml4.add(index))
                };

                if (entry & 1) != 0 {
                    present_count += 1;
                }

                index += 1;
            }

            serial_write("[MM] PML4 entries inspected: 512\r\n");
            serial_write("[MM] Present PML4 entries: ");
            serial_write_u64(present_count);
            serial_write("\r\n");
            serial_write("[MM] PML4 root inspection completed.\r\n");

            // Inspect present entries without dereferencing child tables.
            serial_write("[MM] Inspecting present PML4 entries...\r\n");

            index = 0;

            while index < 512 {
                let entry = unsafe {
                    core::ptr::read_volatile(pml4.add(index))
                };

                if (entry & 1) != 0 {
                    serial_write("[MM] PML4 index: ");
                    serial_write_u64(index as u64);

                    serial_write(" | Raw entry: ");
                    serial_write_u64(entry);

                    serial_write("\r\n[MM] Writable: ");
                    serial_write_u64((entry >> 1) & 1);

                    serial_write(" | User: ");
                    serial_write_u64((entry >> 2) & 1);

                    serial_write(" | NX: ");
                    serial_write_u64((entry >> 63) & 1);
                    serial_write("\r\n");

                    if (entry & (1 << 7)) != 0 {
                        serial_write("[MM] ERROR: Reserved PS bit is set.\r\n");
                    } else {
                        match physical_address_width()
                            .and_then(physical_address_mask)
                        {
                            Some(address_mask) => {
                                let address_bits =
                                    entry & 0x000F_FFFF_FFFF_F000;

                                if (address_bits & !address_mask) != 0 {
                                    serial_write(
                                        "[MM] ERROR: Unsupported physical address bits.\r\n"
                                    );
                                } else {
                                    let child_physical = address_bits & address_mask;

                                    if child_physical == 0 {
                                        serial_write(
                                            "[MM] ERROR: Child table address is zero.\r\n"
                                        );
                                    } else {
                                        serial_write("[MM] Child table physical address: ");
                                        serial_write_u64(child_physical);
                                        serial_write("\r\n");

                                        match child_physical.checked_add(limine::PAGE_SIZE) {
                                            Some(child_end) => {
                                                let mut classified = false;
                                                let mut map_index = 0;

                                                if let Some((_, count, _)) =
                                                    limine::memmap_response_info()
                                                {
                                                    while map_index < count {
                                                        if let Some((base, length, kind)) =
                                                            limine::memmap_entry_at(map_index)
                                                        {
                                                            if let Some(end) = base.checked_add(length) {
                                                                if child_physical >= base
                                                                    && child_end <= end
                                                                {
                                                                    serial_write(
                                                                        "[MM] Child table memory type: "
                                                                    );
                                                                    serial_write_u64(kind);
                                                                    serial_write("\r\n");

                                                                    classified = true;
                                                                    break;
                                                                }
                                                            }
                                                        }

                                                        map_index += 1;
                                                    }
                                                }

                                                if !classified {
                                                    serial_write(
                                                        "[MM] Child table range not classified.\r\n"
                                                    );
                                                }
                                            }

                                            None => {
                                                serial_write(
                                                    "[MM] Child table range overflow.\r\n"
                                                );
                                            }
                                        }
                                    }
                                }
                            }

                            None => {
                                serial_write(
                                    "[MM] ERROR: Physical address mask unavailable.\r\n"
                                );
                            }
                        }
                    }
                }

                index += 1;
            }

            serial_write(
                "[MM] Present PML4 entry inspection completed.\r\n"
            );
        }

        None => {
            serial_write("[MM] PML4 HHDM conversion failed.\r\n");
        }
    }


    
    
    // Physical memory statistics
    match limine::calculate_memmap_stats() {
        Ok(stats) => {
            serial_write("[MEM] Physical memory statistics:\r\n");

            serial_write("[MEM] Total usable bytes: ");
            serial_write_u64(stats.usable_bytes);
            serial_write("\r\n");

            serial_write("[MEM] Usable regions: ");
            serial_write_u64(stats.usable_region_count);
            serial_write("\r\n");

            serial_write("[MEM] Largest usable region: ");
            serial_write_u64(stats.largest_usable_region);
            serial_write("\r\n");

            serial_write("[MEM] Bootloader-reclaimable bytes: ");
            serial_write_u64(stats.bootloader_reclaimable_bytes);
            serial_write("\r\n");

            serial_write("[MEM] Bootloader-reclaimable regions: ");
            serial_write_u64(stats.bootloader_reclaimable_region_count);
            serial_write("\r\n");
        }

        Err(_) => {
            serial_write("[MEM] Statistics calculation FAILED.\r\n");

            loop {
                core::hint::spin_loop();
            }
        }
    }

    // Read Limine memory-map response
    match limine::memmap_response_info() {
        Some((revision, entry_count, _entries)) => {
            serial_write("[MEM] Response received.\r\n");

            serial_write("[MEM] Response revision: ");
            serial_write_u64(revision);

            serial_write("\r\n[MEM] Entry count: ");
            serial_write_u64(entry_count);
            serial_write("\r\n");

            // Read individual memory-map entries
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

    // Kernel banner
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
