
use crate::limine;

pub const MAX_USABLE_REGIONS: usize = 128;

#[derive(Clone, Copy)]
pub struct UsableRegion {
    pub physical_start: u64,
    pub frame_count: u64,
    pub first_frame_index: u64,
}

impl UsableRegion {
    pub const EMPTY: Self = Self {
        physical_start: 0,
        frame_count: 0,
        first_frame_index: 0,
    };
}

pub struct PmmLayout {
    pub regions: [UsableRegion; MAX_USABLE_REGIONS],
    pub region_count: usize,
    pub total_frames: u64,
    pub bitmap_bytes: u64,
    pub bitmap_pages: u64,
    pub bitmap_physical_start: u64,
    pub bitmap_region_index: usize,
    pub bitmap_first_frame_index: u64,
    pub planned_allocatable_frames: u64,
    pub kernel_start: u64,
    pub kernel_end: u64,
    pub bootloader_reclaimable_bytes: u64,
    pub bootloader_reclaimable_regions: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PmmError {
    MissingMemoryMap,
    InvalidMemoryMap,
    ArithmeticOverflow,
    NoUsableFrames,
    TooManyRegions,
    MissingKernelReservation,
    KernelOverlapsUsableMemory,
    NoBitmapBackingRegion,
    BitmapBackingNotMapped,
}

pub fn discover_usable_layout() -> Result<PmmLayout, PmmError> {
    limine::validate_memmap()
        .map_err(|_| PmmError::InvalidMemoryMap)?;

    let (kernel_start, kernel_end) =
        limine::kernel_image_reservation()
            .map_err(|_| PmmError::MissingKernelReservation)?;

    let (_, entry_count, _) =
        limine::memmap_response_info()
            .ok_or(PmmError::MissingMemoryMap)?;

    let mut layout = PmmLayout {
        regions: [UsableRegion::EMPTY; MAX_USABLE_REGIONS],
        region_count: 0,
        total_frames: 0,
        bitmap_bytes: 0,
        bitmap_pages: 0,
        bitmap_physical_start: 0,
        bitmap_region_index: 0,
        bitmap_first_frame_index: 0,
        planned_allocatable_frames: 0,
        kernel_start,
        kernel_end,
        bootloader_reclaimable_bytes: 0,
        bootloader_reclaimable_regions: 0,
    };

    let mut index = 0u64;

    while index < entry_count {
        let (base, length, entry_type) =
            limine::memmap_entry_at(index)
                .ok_or(PmmError::InvalidMemoryMap)?;

        let end = base.checked_add(length)
            .ok_or(PmmError::ArithmeticOverflow)?;

        if entry_type == limine::LIMINE_MEMMAP_BOOTLOADER_RECLAIMABLE {
            layout.bootloader_reclaimable_bytes =
                layout.bootloader_reclaimable_bytes
                    .checked_add(length)
                    .ok_or(PmmError::ArithmeticOverflow)?;

            layout.bootloader_reclaimable_regions += 1;
        }

        if entry_type == limine::LIMINE_MEMMAP_USABLE {
            // The kernel must never overlap a usable region.
            if base < kernel_end && kernel_start < end {
                return Err(PmmError::KernelOverlapsUsableMemory);
            }

            let aligned_start = base
                .checked_add(limine::PAGE_SIZE - 1)
                .ok_or(PmmError::ArithmeticOverflow)?
                & !(limine::PAGE_SIZE - 1);

            let aligned_end =
                end & !(limine::PAGE_SIZE - 1);

            if aligned_start < aligned_end {
                if layout.region_count >= MAX_USABLE_REGIONS {
                    return Err(PmmError::TooManyRegions);
                }

                let frames =
                    (aligned_end - aligned_start)
                    / limine::PAGE_SIZE;

                layout.regions[layout.region_count] = UsableRegion {
                    physical_start: aligned_start,
                    frame_count: frames,
                    first_frame_index: layout.total_frames,
                };

                layout.region_count += 1;

                layout.total_frames = layout.total_frames
                    .checked_add(frames)
                    .ok_or(PmmError::ArithmeticOverflow)?;
            }
        }

        index += 1;
    }

    if layout.total_frames == 0 {
        return Err(PmmError::NoUsableFrames);
    }

    // Calculate bitmap storage requirements.
    layout.bitmap_bytes = layout.total_frames
        .checked_add(7)
        .ok_or(PmmError::ArithmeticOverflow)?
        / 8;

    layout.bitmap_pages = layout.bitmap_bytes
        .checked_add(limine::PAGE_SIZE - 1)
        .ok_or(PmmError::ArithmeticOverflow)?
        / limine::PAGE_SIZE;

    let bitmap_storage_bytes = layout.bitmap_pages
        .checked_mul(limine::PAGE_SIZE)
        .ok_or(PmmError::ArithmeticOverflow)?;

    // Find the largest usable region that can hold the bitmap.
    let mut selected_region: Option<usize> = None;
    let mut largest_frames = 0u64;

    let mut region_index = 0usize;

    while region_index < layout.region_count {
        let region = layout.regions[region_index];

        if region.frame_count >= layout.bitmap_pages
            && region.frame_count > largest_frames
        {
            selected_region = Some(region_index);
            largest_frames = region.frame_count;
        }

        region_index += 1;
    }

    let bitmap_region_index =
        selected_region.ok_or(PmmError::NoBitmapBackingRegion)?;

    let bitmap_region = layout.regions[bitmap_region_index];

    let bitmap_end = bitmap_region.physical_start
        .checked_add(bitmap_storage_bytes)
        .ok_or(PmmError::ArithmeticOverflow)?;

    // Validate the entire bitmap backing range through the HHDM.
    if limine::physical_range_to_virtual(
        bitmap_region.physical_start,
        bitmap_storage_bytes,
    ).is_none() {
        return Err(PmmError::BitmapBackingNotMapped);
    }

    // Store the reservation plan. Do not write to this memory yet.
    layout.bitmap_physical_start = bitmap_region.physical_start;
    layout.bitmap_region_index = bitmap_region_index;
    layout.bitmap_first_frame_index = bitmap_region.first_frame_index;

    // The bitmap's backing frames will not be allocatable.
    layout.planned_allocatable_frames = layout.total_frames
        .checked_sub(layout.bitmap_pages)
        .ok_or(PmmError::ArithmeticOverflow)?;

    // Ensure the selected backing range remains inside its region.
    let region_end = bitmap_region.physical_start
        .checked_add(
            bitmap_region.frame_count
                .checked_mul(limine::PAGE_SIZE)
                .ok_or(PmmError::ArithmeticOverflow)?
        )
        .ok_or(PmmError::ArithmeticOverflow)?;

    if bitmap_end > region_end {
        return Err(PmmError::NoBitmapBackingRegion);
    }

    Ok(layout)
}
