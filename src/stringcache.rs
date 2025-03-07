use super::bumpalloc::LeakyBumpAlloc;

// StringCache stores a Vec of pointers to the StringCacheEntry structs. The
// actual memory for the StringCacheEntry is stored in the LeakyBumpAlloc, and
// each Alloc is rotated out when it's full and a new one twice its size is
// allocated. The Allocator memory is never freed so our strings essentialy have
// a 'static lifetime.
//
// The actual memory representation is as follows. Each StringCacheEntry is
// aligned to 8 bytes on a 64-bit system. The 64-bit memoized hash of the string
// is stored first, then a usize length, then the u8 characters, followed by a
// null terminator (not included in len), then x<8 bytes of uninitialized memory
// as padding before the next aligned entry.
//
//       hash             len       H e l l o , W o r l d !\0
// |. . . . . . . .|. . . . . . . .|. . . . . . . .|. . . . . . . .|
// 0               8               16                     len
// ^ StringCacheEntry              ^ u8 chars               ^ null ^ Next entry
//
// Proper alignment is guaranteed when allocating each entry as the alignment
// is baked into the allocator. StringCache is responsible for monitoring the
// Allocator and creating a new one when it would overflow - the Alloc itself
// will just abort() if it runs out of memory. Note that we abort() rather than
// panic because the behaviour of the spinlock in case of a panic while holding
// the lock is undefined.
//
// Thread safety is ensured because we can only access the StringCache through
// the spinlock in the lazy_static ref. The initial capacity of the cache is
// divided evenly among a number of 'bins' or shards each with their own lock,
// in order to reduce contention.
#[repr(align(128))]
pub(crate) struct StringCache {
    pub(crate) alloc: LeakyBumpAlloc,
    pub(crate) old_allocs: Vec<LeakyBumpAlloc>,
    entries: Vec<*mut StringCacheEntry>,
    num_entries: usize,
    mask: usize,
    total_allocated: usize,
    // padding and aligning to 128 bytes gives up to 20% performance improvement
    _pad: [u32; 3],
}

#[cfg(not(target_pointer_width = "64"))]
const POINTER_SIZE_ERROR: () = "ustr only works on 64-bit architectures";

// TODO: make these configurable?
// Initial size of the StringCache table
pub(crate) const INITIAL_CAPACITY: usize = 1 << 20;
// Initial size of the allocator storage (in bytes)
pub(crate) const INITIAL_ALLOC: usize = 4 << 20;
// Number of bins (shards) for map
pub(crate) const BIN_SHIFT: usize = 6;
pub(crate) const NUM_BINS: usize = 1 << BIN_SHIFT;
// Shift for top bits to determine bin a hash falls into
pub(crate) const TOP_SHIFT: usize = 8 * std::mem::size_of::<usize>() - BIN_SHIFT;

impl StringCache {
    /// Create a new StringCache with the given starting capacity
    pub fn new() -> StringCache {
        let capacity = INITIAL_CAPACITY / NUM_BINS;
        StringCache {
            // current allocator
            alloc: LeakyBumpAlloc::new(
                INITIAL_ALLOC / NUM_BINS,
                std::mem::align_of::<StringCacheEntry>(),
            ),
            // old allocators we'll keep around for iteration purposes.
            // 16 would mean we've allocated 128GB of string storage since we
            // double each time.
            old_allocs: Vec::with_capacity(16),
            // Vector of pointers to the StringCacheEntry headers
            entries: vec![std::ptr::null_mut(); capacity],
            num_entries: 0,
            mask: capacity - 1,
            total_allocated: capacity,
            _pad: [0u32; 3],
        }
    }

    // Insert the given string with its given hash into the cache
    pub(crate) fn insert(&mut self, string: &str, hash: u64) -> *const u8 {
        let mut pos = self.mask & hash as usize;
        let mut dist = 0;
        loop {
            let entry = unsafe { self.entries.get_unchecked(pos) };
            if entry.is_null() {
                // found empty slot to insert
                break;
            }

            // This is safe as long as entry points to a valid address and the
            // layout described in the StringCache doc comment holds.
            unsafe {
                // entry is a *StringCacheEntry so offseting by 1 gives us a
                // pointer to the end of the entry, aka the beginning of the
                // chars.
                // As long as the memory is valid and the layout is correct,
                // we're safe to create a string slice from the chars since
                // they were copied directly from a valid &str.
                let entry_chars = entry.offset(1isize) as *const u8;
                // if entry is non-null then it must point to a valid
                // StringCacheEntry
                let sce = &**entry;
                if sce.hash == hash
                    && sce.len == string.len()
                    && std::str::from_utf8_unchecked(std::slice::from_raw_parts(
                        entry_chars,
                        sce.len,
                    )) == string
                {
                    // found matching string in the cache already, return it
                    return entry_chars;
                }
            }

            // keep looking
            dist += 1;
            debug_assert!(dist <= self.mask);
            pos = (pos + dist) & self.mask;
        }

        //
        // insert the new string
        //

        // we know pos is in bounds as it's &ed with the mask above
        let entry_ptr = unsafe { self.entries.get_unchecked_mut(pos) };
        // add one to length for null byte
        // there's no way we could overflow here in practice since that would
        // require having allocated a u64::MAX-length string, by which time
        // we'll be using 128-bit pointers and we'll need to rewrite this
        // crate anyway
        let byte_len = string.len() + 1;
        let alloc_size = std::mem::size_of::<StringCacheEntry>() + byte_len;

        // if our new allocation would spill over the allocator, make a new
        // allocator and let the old one leak
        let capacity = self.alloc.capacity();
        let allocated = self.alloc.allocated();

        // Ensure we won't overflow - this would be impossible anyway since
        // we can't actually allocate that much memory
        assert!(std::usize::MAX - allocated > alloc_size);

        if alloc_size + allocated > capacity {
            // just in case, make sure we'll definitely have enough storage
            // for the new string.
            assert!(capacity < std::usize::MAX / 2);
            let new_capacity = (capacity * 2).max(alloc_size);
            let old_alloc = std::mem::replace(
                &mut self.alloc,
                LeakyBumpAlloc::new(new_capacity, std::mem::align_of::<StringCacheEntry>()),
            );
            self.old_allocs.push(old_alloc);
            self.total_allocated += new_capacity;
        }

        // This is safe as long as:
        // 1) alloc_size is calculated correctly
        // 2) there is enough space in the allocator (checked in the block above)
        // 3) The StringCacheEntry layout descibed above holds and the memory
        //    returned by allocate() is prooperly aligned.
        unsafe {
            *entry_ptr = self.alloc.allocate(alloc_size) as *mut StringCacheEntry;

            // write the header
            // entry_ptr is guaranteed to point to a valid StringCacheEntry, or
            // alloc.allocate() would have aborted
            std::ptr::write(
                *entry_ptr,
                StringCacheEntry {
                    hash,
                    len: string.len(),
                },
            );
            // write the characters after the StringCacheEntry
            let char_ptr = entry_ptr.offset(1isize) as *mut u8;
            std::ptr::copy_nonoverlapping(string.as_bytes().as_ptr(), char_ptr, string.len());
            // write the trailing null
            let write_ptr = char_ptr.offset(string.len() as isize);
            std::ptr::write(write_ptr, 0u8);

            self.num_entries += 1;
            // we want to keep an 0.5 load factor for the map, so grow if we've
            // exceeded that
            if self.num_entries * 2 > self.mask {
                self.grow();
            }

            char_ptr
        }
    }

    // Double the size of the map storage
    // This is safe as long as
    // - The in-memory layout of the StringCacheEntry is correct
    // If there's not enough memory for the new entry table, it will just abort
    pub(crate) unsafe fn grow(&mut self) {
        let new_mask = self.mask * 2 + 1;
        let mut new_entries = vec![std::ptr::null_mut() as *mut StringCacheEntry; new_mask + 1];
        // copy the existing map into the new map
        let mut to_copy = self.num_entries;
        for e in self.entries.iter_mut() {
            if e.is_null() {
                continue;
            }

            let hash = *(*e as *const u64);
            let mut pos = (hash as usize) & new_mask;
            let mut dist = 0;
            loop {
                if new_entries[pos].is_null() {
                    // here's an empty slot to put the pointer in
                    break;
                }

                dist += 1;
                // This should be impossble as we've allocated twice as many
                // slots as we have entries
                debug_assert!(dist <= new_mask, "Probing wrapped around");
                pos = pos.wrapping_add(dist) & new_mask;
            }

            new_entries[pos] = *e;
            to_copy -= 1;
            if to_copy == 0 {
                break;
            }
        }

        self.entries = new_entries;
        self.mask = new_mask;
    }

    // This is only called by clear() during tests to clear the cache between
    // runs. DO NOT CALL THIS.
    pub(crate) unsafe fn clear(&mut self) {
        // just zero all the pointers that have already been set
        std::ptr::write_bytes(self.entries.as_mut_ptr(), 0, self.mask + 1);
        self.num_entries = 0;
        for a in self.old_allocs.iter_mut() {
            a.clear();
        }
        self.old_allocs.clear();
        self.alloc.clear();
        self.alloc = LeakyBumpAlloc::new(
            INITIAL_ALLOC / NUM_BINS,
            std::mem::align_of::<StringCacheEntry>(),
        );
    }

    pub(crate) fn total_allocated(&self) -> usize {
        self.alloc.allocated() + self.old_allocs.iter().map(|a| a.allocated()).sum::<usize>()
    }

    pub(crate) fn total_capacity(&self) -> usize {
        self.alloc.capacity() + self.old_allocs.iter().map(|a| a.capacity()).sum::<usize>()
    }

    pub(crate) fn num_entries(&self) -> usize {
        self.num_entries
    }
}

impl Default for StringCache {
    fn default() -> StringCache {
        StringCache::new()
    }
}

// We are safe to be Send but not Sync (we get Sync by wrapping in a mutex)
unsafe impl Send for StringCache {}

#[doc(hidden)]
pub struct StringCacheIterator {
    pub(crate) allocs: Vec<(*const u8, *const u8)>,
    pub(crate) current_alloc: usize,
    pub(crate) current_ptr: *const u8,
}

fn round_up_to(n: usize, align: usize) -> usize {
    debug_assert!(align.is_power_of_two());
    (n.checked_add(align).expect("round_up_to overflowed") - 1) & !(align - 1)
}

impl Iterator for StringCacheIterator {
    type Item = &'static str;
    fn next(&mut self) -> Option<Self::Item> {
        let (_, end) = self.allocs[self.current_alloc];
        if self.current_ptr >= end {
            // we've reached the end of the current alloc
            if self.current_alloc == self.allocs.len() - 1 {
                // we've reached the end
                return None;
            } else {
                // advance to the next alloc
                self.current_alloc += 1;
                let (current_ptr, _) = self.allocs[self.current_alloc];
                self.current_ptr = current_ptr;
            }
        }

        // start of the StringCacheEntry is the hash
        unsafe {
            let hash_ptr = self.current_ptr as *const u64;
            let len_ptr = hash_ptr.offset(1) as *const usize;
            let len = *len_ptr;
            let char_ptr = len_ptr.offset(1) as *const u8;
            // the next entry will be the size of the number of bytes in the
            // string, +1 for the null byte, rounded up to the alignment (8)
            self.current_ptr = char_ptr
                .offset(round_up_to(len + 1, std::mem::align_of::<StringCacheEntry>()) as isize);

            let s = std::str::from_utf8_unchecked(std::slice::from_raw_parts(char_ptr, len));
            Some(s)
        }
    }
}

#[repr(C)]
#[derive(Clone)]
struct StringCacheEntry {
    hash: u64,
    len: usize,
}
