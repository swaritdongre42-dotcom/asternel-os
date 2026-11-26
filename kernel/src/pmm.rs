
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

#[derive(Clone, Copy)]
pub struct BitmapStats {
    pub free_frames: u64,
    pub reserved_frames: u64,
    pub virtual_address: u64,
}

pub struct PmmAllocator {
    bitmap: *mut u8,
    bitmap_len: usize,
    next_fit_frame: u64,
    bitmap_physical_start: u64,
    bitmap_pages: u64,
    total_frames: u64,
    free_frames: u64,
    regions: [UsableRegion; MAX_USABLE_REGIONS],
    region_count: usize,
    reserved_low_memory_frames: u64,
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
    InvalidBitmapLayout,
       BitmapValidationFailed,
    InvalidFrameAddress,
    FrameNotUsable,
    FrameAlreadyFree,
    ReservedFrame,
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

            layout.bootloader_reclaimable_regions =
                layout.bootloader_reclaimable_regions
                    .checked_add(1)
                    .ok_or(PmmError::ArithmeticOverflow)?;
        }

        if entry_type == limine::LIMINE_MEMMAP_USABLE {
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

    if limine::physical_range_to_virtual(
        bitmap_region.physical_start,
        bitmap_storage_bytes,
    ).is_none() {
        return Err(PmmError::BitmapBackingNotMapped);
    }

    layout.bitmap_physical_start = bitmap_region.physical_start;
    layout.bitmap_region_index = bitmap_region_index;
    layout.bitmap_first_frame_index = bitmap_region.first_frame_index;

    layout.planned_allocatable_frames = layout.total_frames
        .checked_sub(layout.bitmap_pages)
        .ok_or(PmmError::ArithmeticOverflow)?;

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

pub fn initialize_bitmap(
    layout: &PmmLayout,
) -> Result<BitmapStats, PmmError> {
    if layout.total_frames == 0
        || layout.region_count == 0
        || layout.region_count > MAX_USABLE_REGIONS
        || layout.bitmap_pages == 0
        || layout.bitmap_bytes == 0
    {
        return Err(PmmError::InvalidBitmapLayout);
    }

    let expected_bitmap_bytes = layout.total_frames
        .checked_add(7)
        .ok_or(PmmError::ArithmeticOverflow)?
        / 8;

    let expected_bitmap_pages = expected_bitmap_bytes
        .checked_add(limine::PAGE_SIZE - 1)
        .ok_or(PmmError::ArithmeticOverflow)?
        / limine::PAGE_SIZE;

    if layout.bitmap_bytes != expected_bitmap_bytes
        || layout.bitmap_pages != expected_bitmap_pages
    {
        return Err(PmmError::InvalidBitmapLayout);
    }

    let storage_bytes = layout.bitmap_pages
        .checked_mul(limine::PAGE_SIZE)
        .ok_or(PmmError::ArithmeticOverflow)?;

    let bitmap_len = usize::try_from(layout.bitmap_bytes)
        .map_err(|_| PmmError::ArithmeticOverflow)?;

    let storage_len = usize::try_from(storage_bytes)
        .map_err(|_| PmmError::ArithmeticOverflow)?;

    let mut expected_first_frame = 0u64;
    let mut index = 0usize;

    while index < layout.region_count {
        let region = layout.regions[index];

        if region.frame_count == 0
            || region.physical_start % limine::PAGE_SIZE != 0
            || region.first_frame_index != expected_first_frame
        {
            return Err(PmmError::InvalidBitmapLayout);
        }

        region.frame_count
            .checked_mul(limine::PAGE_SIZE)
            .ok_or(PmmError::ArithmeticOverflow)?;

        expected_first_frame = expected_first_frame
            .checked_add(region.frame_count)
            .ok_or(PmmError::ArithmeticOverflow)?;

        index += 1;
    }

    if expected_first_frame != layout.total_frames {
        return Err(PmmError::InvalidBitmapLayout);
    }

    if layout.bitmap_region_index >= layout.region_count {
        return Err(PmmError::InvalidBitmapLayout);
    }

    let backing_region =
        layout.regions[layout.bitmap_region_index];

    if backing_region.physical_start != layout.bitmap_physical_start
        || backing_region.first_frame_index
            != layout.bitmap_first_frame_index
        || layout.bitmap_pages > backing_region.frame_count
    {
        return Err(PmmError::InvalidBitmapLayout);
    }

    let expected_allocatable = layout.total_frames
        .checked_sub(layout.bitmap_pages)
        .ok_or(PmmError::InvalidBitmapLayout)?;

    if layout.planned_allocatable_frames != expected_allocatable {
        return Err(PmmError::InvalidBitmapLayout);
    }

    let bitmap_ptr = limine::physical_range_to_virtual(
        layout.bitmap_physical_start,
        storage_bytes,
    )
    .ok_or(PmmError::BitmapBackingNotMapped)?;

    if bitmap_ptr.is_null() {
        return Err(PmmError::BitmapBackingNotMapped);
    }

    let virtual_address = bitmap_ptr as u64;

    let bitmap = unsafe {
        core::slice::from_raw_parts_mut(
            bitmap_ptr,
            storage_len,
        )
    };

    bitmap.fill(0xFF);

    index = 0;

    while index < layout.region_count {
        let region = layout.regions[index];
        let mut offset = 0u64;

        while offset < region.frame_count {
            let frame_index =
                region.first_frame_index + offset;

            let byte_index = (frame_index / 8) as usize;
            let bit_index = (frame_index % 8) as u8;

            bitmap[byte_index] &= !(1u8 << bit_index);

            offset += 1;
        }

        index += 1;
    }

    let mut reserved_offset = 0u64;

    while reserved_offset < layout.bitmap_pages {
        let frame_index =
            layout.bitmap_first_frame_index + reserved_offset;

        let byte_index = (frame_index / 8) as usize;
        let bit_index = (frame_index % 8) as u8;

        bitmap[byte_index] |= 1u8 << bit_index;

        reserved_offset += 1;
    }

    let remaining_bits = (layout.total_frames % 8) as u8;

    if remaining_bits != 0 {
        let last_byte = bitmap[bitmap_len - 1];

        let unused_mask = !((1u8 << remaining_bits) - 1);

        if last_byte & unused_mask != unused_mask {
            return Err(PmmError::BitmapValidationFailed);
        }
    }

    let mut free_frames = 0u64;
    let mut frame_index = 0u64;

    while frame_index < layout.total_frames {
        let byte_index = (frame_index / 8) as usize;
        let bit_index = (frame_index % 8) as u8;

        if bitmap[byte_index] & (1u8 << bit_index) == 0 {
            free_frames = free_frames
                .checked_add(1)
                .ok_or(PmmError::ArithmeticOverflow)?;
        }

        frame_index += 1;
    }

    let reserved_frames = layout.total_frames
        .checked_sub(free_frames)
        .ok_or(PmmError::ArithmeticOverflow)?;

    if free_frames != layout.planned_allocatable_frames
        || reserved_frames != layout.bitmap_pages
    {
        return Err(PmmError::BitmapValidationFailed);
    }

    Ok(BitmapStats {
        free_frames,
        reserved_frames,
        virtual_address,
    })
}

impl PmmAllocator {
    pub fn from_initialized_layout(
        layout: &PmmLayout,
        stats: BitmapStats,
       ) -> Result<Self, PmmError> {
        if layout.total_frames == 0
            || layout.region_count == 0
            || layout.region_count > MAX_USABLE_REGIONS
            || stats.free_frames != layout.planned_allocatable_frames
            || stats.reserved_frames != layout.bitmap_pages
        {
            return Err(PmmError::InvalidBitmapLayout);
        }

        let total_count = stats.free_frames
            .checked_add(stats.reserved_frames)
            .ok_or(PmmError::ArithmeticOverflow)?;

        if total_count != layout.total_frames {
            return Err(PmmError::InvalidBitmapLayout);
        }

        let storage_bytes = layout.bitmap_pages
            .checked_mul(limine::PAGE_SIZE)
            .ok_or(PmmError::ArithmeticOverflow)?;

        let bitmap_len = usize::try_from(layout.bitmap_bytes)
            .map_err(|_| PmmError::ArithmeticOverflow)?;

        let bitmap_ptr = limine::physical_range_to_virtual(
            layout.bitmap_physical_start,
            storage_bytes,
        )
        .ok_or(PmmError::BitmapBackingNotMapped)?;

        if bitmap_ptr.is_null()
            || bitmap_ptr as u64 != stats.virtual_address
        {
            return Err(PmmError::BitmapBackingNotMapped);
        }

        let mut regions = [UsableRegion::EMPTY; MAX_USABLE_REGIONS];

        let mut index = 0usize;

        while index < layout.region_count {
            regions[index] = layout.regions[index];
            index += 1;
        }

        let mut free_frames = stats.free_frames;
        let mut reserved_low_memory_frames = 0u64;

        const LOW_MEMORY_RESERVED_END: u64 = 0x0010_0000;

        // Reserve usable frames below the first 1 MiB.
        let mut region_index = 0usize;

        while region_index < layout.region_count {
            let region = regions[region_index];
            let mut offset = 0u64;

            while offset < region.frame_count {
                let physical_address = region.physical_start
                    .checked_add(
                        offset.checked_mul(limine::PAGE_SIZE)
                            .ok_or(PmmError::ArithmeticOverflow)?
                    )
                    .ok_or(PmmError::ArithmeticOverflow)?;

                if physical_address >= LOW_MEMORY_RESERVED_END {
                    break;
                }

                let frame_index = region.first_frame_index
                    .checked_add(offset)
                    .ok_or(PmmError::ArithmeticOverflow)?;

                let byte_index = usize::try_from(frame_index / 8)
                    .map_err(|_| PmmError::ArithmeticOverflow)?;

                if byte_index >= bitmap_len {
                    return Err(PmmError::InvalidBitmapLayout);
                }

                let bit_mask = 1u8 << (frame_index % 8);

                let current = unsafe {
                    bitmap_ptr.add(byte_index).read()
                };

                if current & bit_mask == 0 {
                    unsafe {
                        bitmap_ptr.add(byte_index)
                            .write(current | bit_mask);
                    }

                    free_frames = free_frames
                        .checked_sub(1)
                        .ok_or(PmmError::BitmapValidationFailed)?;

                    reserved_low_memory_frames =
                        reserved_low_memory_frames
                            .checked_add(1)
                            .ok_or(PmmError::ArithmeticOverflow)?;
                }

                offset += 1;
            }

            region_index += 1;
        }

        Ok(Self {
            bitmap: bitmap_ptr,
            bitmap_len,
            total_frames: layout.total_frames,
            free_frames,
            regions,
            region_count: layout.region_count,
            reserved_low_memory_frames,
         bitmap_physical_start: layout.bitmap_physical_start,
            bitmap_pages: layout.bitmap_pages,
            next_fit_frame: 0,
        })
    }

    pub fn frame_index_to_physical(
        &self,
        frame_index: u64,
    ) -> Result<u64, PmmError> {
        if frame_index >= self.total_frames {
            return Err(PmmError::InvalidBitmapLayout);
        }

        let mut index = 0usize;

        while index < self.region_count {
            let region = self.regions[index];

            let region_end = region.first_frame_index
                .checked_add(region.frame_count)
                .ok_or(PmmError::ArithmeticOverflow)?;

            if frame_index >= region.first_frame_index
                && frame_index < region_end
            {
                let offset = frame_index
                    .checked_sub(region.first_frame_index)
                    .ok_or(PmmError::InvalidBitmapLayout)?;

                let physical_offset = offset
                    .checked_mul(limine::PAGE_SIZE)
                    .ok_or(PmmError::ArithmeticOverflow)?;

                let physical_address = region.physical_start
                    .checked_add(physical_offset)
                    .ok_or(PmmError::ArithmeticOverflow)?;

                if physical_address % limine::PAGE_SIZE != 0 {
                    return Err(PmmError::InvalidBitmapLayout);
                }

                return Ok(physical_address);
            }

            index += 1;
        }

        Err(PmmError::InvalidBitmapLayout)
    }

   
    // Find a free frame in the half-open range [start_frame, end_frame).
    // Scans bitmap bytes and skips fully allocated bytes.
    fn find_free_frame_in_range(
        &self,
        start_frame: u64,
        end_frame: u64,
    ) -> Result<Option<u64>, PmmError> {
        if start_frame >= end_frame {
            return Ok(None);
        }

        let mut byte_index_u64 = start_frame / 8;

        let end_byte = end_frame
            .checked_add(7)
            .ok_or(PmmError::ArithmeticOverflow)?
            / 8;

        while byte_index_u64 < end_byte {
            let byte_index = usize::try_from(byte_index_u64)
                .map_err(|_| PmmError::ArithmeticOverflow)?;

            if byte_index >= self.bitmap_len {
                return Err(PmmError::InvalidBitmapLayout);
            }

            let current = unsafe {
                self.bitmap.add(byte_index).read()
            };

            // Entirely allocated bytes cannot contain a free frame.
            if current != 0xFF {
                let byte_start = byte_index_u64
                    .checked_mul(8)
                    .ok_or(PmmError::ArithmeticOverflow)?;

                let range_start = start_frame.max(byte_start);
                let range_end = end_frame.min(
                    byte_start
                        .checked_add(8)
                        .ok_or(PmmError::ArithmeticOverflow)?
                );

                let first_bit = (range_start - byte_start) as u32;
                let end_bit = (range_end - byte_start) as u32;

                let lower_mask = (1u16 << first_bit) - 1;
                let upper_mask = (1u16 << end_bit) - 1;

                let valid_mask = (upper_mask & !lower_mask) as u8;

                let free_bits = !current & valid_mask;

                if free_bits != 0 {
                    let bit_index = free_bits.trailing_zeros() as u64;

                    let frame_index = byte_start
                        .checked_add(bit_index)
                        .ok_or(PmmError::ArithmeticOverflow)?;

                    return Ok(Some(frame_index));
                }
            }

            byte_index_u64 += 1;
        }

        Ok(None)
    }

    pub fn allocate_frame(
        &mut self,
    ) -> Result<Option<u64>, PmmError> {
        if self.free_frames == 0 {
            return Ok(None);
        }

        if self.total_frames == 0
            || self.next_fit_frame >= self.total_frames
        {
            return Err(PmmError::InvalidBitmapLayout);
        }

        // First search: cursor to the end of the bitmap.
        let mut found = self.find_free_frame_in_range(
            self.next_fit_frame,
            self.total_frames,
        )?;

        // Second search: wrap around to the beginning.
        if found.is_none() && self.next_fit_frame != 0 {
            found = self.find_free_frame_in_range(
                0,
                self.next_fit_frame,
            )?;
        }

        let frame_index =
            found.ok_or(PmmError::BitmapValidationFailed)?;

        let byte_index = usize::try_from(frame_index / 8)
            .map_err(|_| PmmError::ArithmeticOverflow)?;

        if byte_index >= self.bitmap_len {
            return Err(PmmError::InvalidBitmapLayout);
        }

        let bit_mask = 1u8 << (frame_index % 8);

        let current = unsafe {
            self.bitmap.add(byte_index).read()
        };

        // Defensive check before modifying the bitmap.
        if current & bit_mask != 0 {
            return Err(PmmError::BitmapValidationFailed);
        }

        let physical_address =
            self.frame_index_to_physical(frame_index)?;

        let remaining_free = self.free_frames
            .checked_sub(1)
            .ok_or(PmmError::BitmapValidationFailed)?;

        unsafe {
            self.bitmap.add(byte_index)
                .write(current | bit_mask);
        }

        self.free_frames = remaining_free;

        // Advance the cursor, wrapping at the bitmap end.
        self.next_fit_frame = if frame_index + 1 >= self.total_frames {
            0
        } else {
            frame_index + 1
        };

        Ok(Some(physical_address))
    }


    // Find a physically contiguous free run inside one usable region.
    // The search bounds constrain the run's starting frame, not its end.
    fn find_contiguous_in_region(
        &self,
        region_index: usize,
        minimum_start: u64,
        maximum_start: u64,
        frame_count: u64,
        alignment_frames: u64,
    ) -> Result<Option<u64>, PmmError> {
        let region = self.regions[region_index];

        let region_end = region.first_frame_index
            .checked_add(region.frame_count)
            .ok_or(PmmError::ArithmeticOverflow)?;

        let mut candidate = minimum_start.max(region.first_frame_index);
        let candidate_limit = maximum_start.min(region_end);

        if candidate >= candidate_limit {
            return Ok(None);
        }

        let physical_first_frame =
            region.physical_start / limine::PAGE_SIZE;

        loop {
            let candidate_offset = candidate
                .checked_sub(region.first_frame_index)
                .ok_or(PmmError::InvalidBitmapLayout)?;

            let physical_candidate = physical_first_frame
                .checked_add(candidate_offset)
                .ok_or(PmmError::ArithmeticOverflow)?;

            let remainder = physical_candidate % alignment_frames;
            if remainder != 0 {
                let adjustment = alignment_frames - remainder;
                candidate = candidate
                    .checked_add(adjustment)
                    .ok_or(PmmError::ArithmeticOverflow)?;
            }

            if candidate >= candidate_limit {
                return Ok(None);
            }

            let run_end = candidate
                .checked_add(frame_count)
                .ok_or(PmmError::ArithmeticOverflow)?;

            if run_end > region_end {
                return Ok(None);
            }

            let mut offset = 0u64;
            let mut occupied_offset = None;

            while offset < frame_count {
                let frame_index = candidate
                    .checked_add(offset)
                    .ok_or(PmmError::ArithmeticOverflow)?;

                let byte_index = usize::try_from(frame_index / 8)
                    .map_err(|_| PmmError::ArithmeticOverflow)?;

                if byte_index >= self.bitmap_len {
                    return Err(PmmError::InvalidBitmapLayout);
                }

                let bit_mask = 1u8 << (frame_index % 8);
                let value = unsafe {
                    self.bitmap.add(byte_index).read()
                };

                if value & bit_mask != 0 {
                    occupied_offset = Some(offset);
                    break;
                }

                offset += 1;
            }

            match occupied_offset {
                None => return Ok(Some(candidate)),
                Some(offset) => {
                    // Every start position up to this occupied frame would
                    // include it, so skip those candidates.
                    candidate = candidate
                        .checked_add(offset)
                        .and_then(|value| value.checked_add(1))
                        .ok_or(PmmError::ArithmeticOverflow)?;
                }
            }
        }
    }

    fn find_contiguous_in_range(
        &self,
        minimum_start: u64,
        maximum_start: u64,
        frame_count: u64,
        alignment_frames: u64,
    ) -> Result<Option<u64>, PmmError> {
        let mut region_index = 0usize;

        while region_index < self.region_count {
            if let Some(frame_index) = self.find_contiguous_in_region(
                region_index,
                minimum_start,
                maximum_start,
                frame_count,
                alignment_frames,
            )? {
                return Ok(Some(frame_index));
            }

            region_index += 1;
        }

        Ok(None)
    }

    pub fn allocate_contiguous(
        &mut self,
        frame_count: u64,
        alignment_frames: u64,
    ) -> Result<Option<u64>, PmmError> {
        if frame_count == 0 || alignment_frames == 0 {
            return Err(PmmError::InvalidFrameAddress);
        }

        if self.total_frames == 0
            || self.next_fit_frame >= self.total_frames
        {
            return Err(PmmError::InvalidBitmapLayout);
        }

        if frame_count > self.free_frames {
            return Ok(None);
        }

        let mut found = self.find_contiguous_in_range(
            self.next_fit_frame,
            self.total_frames,
            frame_count,
            alignment_frames,
        )?;

        if found.is_none() && self.next_fit_frame != 0 {
            found = self.find_contiguous_in_range(
                0,
                self.next_fit_frame,
                frame_count,
                alignment_frames,
            )?;
        }

        let first_frame = match found {
            Some(index) => index,
            None => return Ok(None),
        };

        let physical_start =
            self.frame_index_to_physical(first_frame)?;

        if (physical_start / limine::PAGE_SIZE)
            % alignment_frames != 0
        {
            return Err(PmmError::InvalidBitmapLayout);
        }

        let new_free_frames = self.free_frames
            .checked_sub(frame_count)
            .ok_or(PmmError::BitmapValidationFailed)?;

        let run_end = first_frame
            .checked_add(frame_count)
            .ok_or(PmmError::ArithmeticOverflow)?;

        // Validate the complete run before changing any bitmap bits.
        let mut offset = 0u64;
        while offset < frame_count {
            let frame_index = first_frame
                .checked_add(offset)
                .ok_or(PmmError::ArithmeticOverflow)?;

            let byte_index = usize::try_from(frame_index / 8)
                .map_err(|_| PmmError::ArithmeticOverflow)?;

            if byte_index >= self.bitmap_len {
                return Err(PmmError::InvalidBitmapLayout);
            }

            let bit_mask = 1u8 << (frame_index % 8);
            let value = unsafe {
                self.bitmap.add(byte_index).read()
            };

            if value & bit_mask != 0 {
                return Err(PmmError::BitmapValidationFailed);
            }

            offset += 1;
        }

        // Mark every frame allocated.
        offset = 0;
        while offset < frame_count {
            let frame_index = first_frame + offset;
            let byte_index = (frame_index / 8) as usize;
            let bit_mask = 1u8 << (frame_index % 8);

            let value = unsafe {
                self.bitmap.add(byte_index).read()
            };

            unsafe {
                self.bitmap.add(byte_index)
                    .write(value | bit_mask);
            }

            offset += 1;
        }

        self.free_frames = new_free_frames;
        self.next_fit_frame = if run_end >= self.total_frames {
            0
        } else {
            run_end
        };

        Ok(Some(physical_start))
    }

    pub fn free_contiguous(
        &mut self,
        physical_start: u64,
        frame_count: u64,
    ) -> Result<(), PmmError> {
        if frame_count == 0
            || physical_start % limine::PAGE_SIZE != 0
        {
            return Err(PmmError::InvalidFrameAddress);
        }

        let range_bytes = frame_count
            .checked_mul(limine::PAGE_SIZE)
            .ok_or(PmmError::ArithmeticOverflow)?;

        let physical_end = physical_start
            .checked_add(range_bytes)
            .ok_or(PmmError::ArithmeticOverflow)?;

        // Protect low memory.
        if physical_start < 0x0010_0000 {
            return Err(PmmError::ReservedFrame);
        }

        // Protect the bitmap's physical backing pages.
        let bitmap_bytes = self.bitmap_pages
            .checked_mul(limine::PAGE_SIZE)
            .ok_or(PmmError::ArithmeticOverflow)?;

        let bitmap_end = self.bitmap_physical_start
            .checked_add(bitmap_bytes)
            .ok_or(PmmError::ArithmeticOverflow)?;

        if physical_start < bitmap_end
            && self.bitmap_physical_start < physical_end
        {
            return Err(PmmError::ReservedFrame);
        }

        // The entire range must belong to one usable region. This prevents
        // treating adjacent bitmap indices from separate regions as one run.
        let mut first_frame_index = None;
        let mut region_index = 0usize;

        while region_index < self.region_count {
            let region = self.regions[region_index];

            let region_bytes = region.frame_count
                .checked_mul(limine::PAGE_SIZE)
                .ok_or(PmmError::ArithmeticOverflow)?;

            let region_end = region.physical_start
                .checked_add(region_bytes)
                .ok_or(PmmError::ArithmeticOverflow)?;

            if physical_start >= region.physical_start
                && physical_end <= region_end
            {
                let byte_offset = physical_start
                    .checked_sub(region.physical_start)
                    .ok_or(PmmError::InvalidFrameAddress)?;

                if byte_offset % limine::PAGE_SIZE != 0 {
                    return Err(PmmError::InvalidFrameAddress);
                }

                let frame_offset = byte_offset / limine::PAGE_SIZE;

                first_frame_index = Some(
                    region.first_frame_index
                        .checked_add(frame_offset)
                        .ok_or(PmmError::ArithmeticOverflow)?
                );

                break;
            }

            region_index += 1;
        }

        let first_frame = first_frame_index
            .ok_or(PmmError::FrameNotUsable)?;

        let run_end = first_frame
            .checked_add(frame_count)
            .ok_or(PmmError::ArithmeticOverflow)?;

        if run_end > self.total_frames {
            return Err(PmmError::FrameNotUsable);
        }

        // Validate every bit first, so double-free and partially-free
        // ranges are reported accurately before accounting checks.
        let mut offset = 0u64;
        while offset < frame_count {
            let frame_index = first_frame
                .checked_add(offset)
                .ok_or(PmmError::ArithmeticOverflow)?;

            let byte_index = usize::try_from(frame_index / 8)
                .map_err(|_| PmmError::ArithmeticOverflow)?;

            if byte_index >= self.bitmap_len {
                return Err(PmmError::InvalidBitmapLayout);
            }

            let bit_mask = 1u8 << (frame_index % 8);
            let value = unsafe {
                self.bitmap.add(byte_index).read()
            };

            if value & bit_mask == 0 {
                return Err(PmmError::FrameAlreadyFree);
            }

            offset += 1;
        }

        // Only fully allocated ranges may increase the free-frame count.
        let new_free_frames = self.free_frames
            .checked_add(frame_count)
            .ok_or(PmmError::ArithmeticOverflow)?;

        if new_free_frames > self.total_frames {
            return Err(PmmError::BitmapValidationFailed);
        }

        // Release the complete range.
        offset = 0;
        while offset < frame_count {
            let frame_index = first_frame + offset;
            let byte_index = (frame_index / 8) as usize;
            let bit_mask = 1u8 << (frame_index % 8);

            let value = unsafe {
                self.bitmap.add(byte_index).read()
            };

            unsafe {
                self.bitmap.add(byte_index)
                    .write(value & !bit_mask);
            }

            offset += 1;
        }

        self.free_frames = new_free_frames;

        Ok(())
    }

    pub fn next_fit_cursor(&self) -> u64 {
    self.next_fit_frame
}

    
   pub fn free_frame(
    &mut self,
    physical_address: u64,
) -> Result<(), PmmError> {
    if physical_address % limine::PAGE_SIZE != 0 {
        return Err(PmmError::InvalidFrameAddress);
    }

    let mut frame_index = None;
    let mut region_index = 0usize;

    while region_index < self.region_count {
        let region = self.regions[region_index];

        let region_bytes = region.frame_count
            .checked_mul(limine::PAGE_SIZE)
            .ok_or(PmmError::ArithmeticOverflow)?;

        let region_end = region.physical_start
            .checked_add(region_bytes)
            .ok_or(PmmError::ArithmeticOverflow)?;

        if physical_address >= region.physical_start
            && physical_address < region_end
        {
            let offset = physical_address
                .checked_sub(region.physical_start)
                .ok_or(PmmError::InvalidFrameAddress)?;

            if offset % limine::PAGE_SIZE != 0 {
                return Err(PmmError::InvalidFrameAddress);
            }

            let frame_offset = offset / limine::PAGE_SIZE;

            frame_index = Some(
                region.first_frame_index
                    .checked_add(frame_offset)
                    .ok_or(PmmError::ArithmeticOverflow)?
            );

            break;
        }

        region_index += 1;
    }

    let frame_index =
        frame_index.ok_or(PmmError::FrameNotUsable)?;

    if frame_index >= self.total_frames {
        return Err(PmmError::InvalidBitmapLayout);
    }

    // Protect reserved low memory.
    if physical_address < 0x0010_0000 {
        return Err(PmmError::ReservedFrame);
    }

    // Protect the physical pages occupied by the bitmap.
    let bitmap_bytes = self.bitmap_pages
        .checked_mul(limine::PAGE_SIZE)
        .ok_or(PmmError::ArithmeticOverflow)?;

    let bitmap_end = self.bitmap_physical_start
        .checked_add(bitmap_bytes)
        .ok_or(PmmError::ArithmeticOverflow)?;

    if physical_address >= self.bitmap_physical_start
        && physical_address < bitmap_end
    {
        return Err(PmmError::ReservedFrame);
    }

    let byte_index = usize::try_from(frame_index / 8)
        .map_err(|_| PmmError::ArithmeticOverflow)?;

    if byte_index >= self.bitmap_len {
        return Err(PmmError::InvalidBitmapLayout);
    }

    let bit_mask = 1u8 << (frame_index % 8);

    let current = unsafe {
        self.bitmap.add(byte_index).read()
    };

    if current & bit_mask == 0 {
        return Err(PmmError::FrameAlreadyFree);
    }

    if self.free_frames >= self.total_frames {
        return Err(PmmError::BitmapValidationFailed);
    }

    unsafe {
        self.bitmap.add(byte_index)
            .write(current & !bit_mask);
    }

    self.free_frames = self.free_frames
        .checked_add(1)
        .ok_or(PmmError::ArithmeticOverflow)?;

    Ok(())
}
    
    pub fn free_frames(&self) -> u64 {
        self.free_frames
    }

    pub fn total_frames(&self) -> u64 {
        self.total_frames
    }

    pub fn reserved_low_memory_frames(&self) -> u64 {
        self.reserved_low_memory_frames
    }
}



pub fn validate_next_fit_wraparound() -> bool {
    // 16 simulated frames.
    // Only frame index 2 is free.
    let mut bitmap = [0xFBu8, 0xFFu8];

    let mut regions = [UsableRegion::EMPTY; MAX_USABLE_REGIONS];

    regions[0] = UsableRegion {
        physical_start: 0x0010_0000,
        frame_count: 16,
        first_frame_index: 0,
    };

    let mut allocator = PmmAllocator {
        bitmap: bitmap.as_mut_ptr(),
        bitmap_len: bitmap.len(),
        next_fit_frame: 12,
        bitmap_physical_start: 0x0080_0000,
        bitmap_pages: 1,
        total_frames: 16,
        free_frames: 1,
        regions,
        region_count: 1,
        reserved_low_memory_frames: 0,
    };

    let allocated = allocator.allocate_frame();

    matches!(
        allocated,
        Ok(Some(0x0010_2000))
    )
        && allocator.next_fit_cursor() == 3
        && allocator.free_frames() == 0
        && bitmap[0] == 0xFF
        && bitmap[1] == 0xFF
}

pub fn validate_contiguous_allocation() -> bool {
    // Two physically discontiguous regions with adjacent bitmap indices.
    let mut bitmap = [0u8; 1];
    let mut regions = [UsableRegion::EMPTY; MAX_USABLE_REGIONS];

    regions[0] = UsableRegion {
        physical_start: 0x0010_0000,
        frame_count: 4,
        first_frame_index: 0,
    };

    regions[1] = UsableRegion {
        physical_start: 0x0030_0000,
        frame_count: 4,
        first_frame_index: 4,
    };

    let mut allocator = PmmAllocator {
        bitmap: bitmap.as_mut_ptr(),
        bitmap_len: bitmap.len(),
        next_fit_frame: 0,
        bitmap_physical_start: 0x0080_0000,
        bitmap_pages: 1,
        total_frames: 8,
        free_frames: 8,
        regions,
        region_count: 2,
        reserved_low_memory_frames: 0,
    };

    let initial_free = allocator.free_frames();

    let allocation = match allocator.allocate_contiguous(2, 4) {
        Ok(Some(address)) => address,
        _ => return false,
    };

    if allocation != 0x0010_0000
        || (allocation / limine::PAGE_SIZE) % 4 != 0
        || allocator.free_frames() != initial_free - 2
        || bitmap[0] & 0b0000_0011 != 0b0000_0011
        || allocation.checked_add(limine::PAGE_SIZE) != Some(0x0010_1000)
    {
        return false;
    }

    if allocator.free_contiguous(allocation, 2).is_err()
        || allocator.free_frames() != initial_free
        || bitmap[0] & 0b0000_0011 != 0
    {
        return false;
    }

    if !matches!(
        allocator.free_contiguous(allocation, 2),
        Err(PmmError::FrameAlreadyFree)
    ) || allocator.free_frames() != initial_free {
        return false;
    }

    if !matches!(
        allocator.allocate_contiguous(0, 1),
        Err(PmmError::InvalidFrameAddress)
    ) || !matches!(
        allocator.allocate_contiguous(1, 0),
        Err(PmmError::InvalidFrameAddress)
    ) || !matches!(
        allocator.free_contiguous(allocation, 0),
        Err(PmmError::InvalidFrameAddress)
    ) {
        return false;
    }

    // A partially free range must be rejected without freeing its
    // still-allocated frames or changing accounting.
    let partial_run = match allocator.allocate_contiguous(2, 1) {
        Ok(Some(address)) => address,
        _ => return false,
    };

    if allocator.free_frame(partial_run).is_err()
        || allocator.free_frames() != initial_free - 1
    {
        return false;
    }

    if !matches!(
        allocator.free_contiguous(partial_run, 2),
        Err(PmmError::FrameAlreadyFree)
    ) || allocator.free_frames() != initial_free - 1 {
        return false;
    }

    if allocator.free_frame(partial_run + limine::PAGE_SIZE).is_err()
        || allocator.free_frames() != initial_free
    {
        return false;
    }

    // Adjacent bitmap indices in separate regions are not contiguous RAM.
    if !matches!(
        allocator.allocate_contiguous(5, 1),
        Ok(None)
    ) || allocator.free_frames() != initial_free {
        return false;
    }

    if !matches!(
        allocator.free_contiguous(0x0010_3000, 2),
        Err(PmmError::FrameNotUsable)
    ) || allocator.free_frames() != initial_free {
        return false;
    }

    if !matches!(
        allocator.free_contiguous(0, 1),
        Err(PmmError::ReservedFrame)
    ) || !matches!(
        allocator.free_contiguous(0x0080_0000, 1),
        Err(PmmError::ReservedFrame)
    ) {
        return false;
    }

    if !matches!(
        allocator.free_contiguous(allocation + 1, 1),
        Err(PmmError::InvalidFrameAddress)
    ) || !matches!(
        allocator.free_contiguous(u64::MAX - (limine::PAGE_SIZE - 1), 2),
        Err(PmmError::ArithmeticOverflow)
    ) {
        return false;
    }

    // Verify next-fit wraparound for a multi-frame run.
    allocator.next_fit_frame = 6;

    let wrapped_run = match allocator.allocate_contiguous(3, 1) {
        Ok(Some(address)) => address,
        _ => return false,
    };

    if wrapped_run != 0x0010_0000
        || allocator.next_fit_cursor() != 3
        || allocator.free_frames() != initial_free - 3
    {
        return false;
    }

    if allocator.free_contiguous(wrapped_run, 3).is_err()
        || allocator.free_frames() != initial_free
    {
        return false;
    }

    let second_region = match allocator.allocate_contiguous(4, 1) {
        Ok(Some(address)) => address,
        _ => return false,
    };

    if second_region != 0x0030_0000
        || allocator.free_frames() != initial_free - 4
    {
        return false;
    }

    if allocator.free_contiguous(second_region, 4).is_err()
        || allocator.free_frames() != initial_free
    {
        return false;
    }

    true
}

