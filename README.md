# Ustr
Fast, FFI-friendly string interning. 

A `Ustr` (**U**nique **str**) is a lightweight handle representing a static, immutable entry in a global string cache, allowing for: 
* Extremely fast string assignment and comparisons 
* Efficient storage. Only one copy of the string is held in memory, and getting access to it is just a pointer indirection.
* Fast hashing - the precomputed hash is stored with the string
* Fast FFI - the string is stored with a terminating null byte so can be passed to C directly without doing the CString dance.

The downside is no strings are ever freed, so if you're creating lots and lots of strings, you might run out of memory. On the other hand, *War and Peace*
is only 3MB, so it's probably fine. 

This crate is based on [OpenImageIO's ustring](https://github.com/OpenImageIO/oiio/blob/master/src/include/OpenImageIO/ustring.h) but it is NOT binary-compatible (yet). The underlying hash map implementation is directy ported from OIIO.

# Usage

```rust
use ustr::{Ustr, ustr};

// Creation is quick and easy using either `Ustr::from` or the `ustr` short 
// function and only one copy of any string is stored
let h1 = Ustr::from("hello");
let h2 = ustr("hello");

// Comparisons and copies are extremely cheap
let h3 = h1;
assert_eq!(h2, h3); 

// You can pass straight to FFI
let len = unsafe {
    libc::strlen(h1.as_char_ptr())
};
assert_eq!(len, 5);

// For best performance when using Ustr as key for a HashMap or HashSet,
// you'll want to use the precomputed hash. To make this easier, just use
// the UstrMap and UstrSet exports:
use ustr::UstrMap;

// Key type is always Ustr
let mut map: UstrMap<usize> = UstrMap::default();
map.insert(u1, 17);
assert_eq!(*map.get(&u1).unwrap(), 17);
```

# Compared to string-cache
[string-cache](https://github.com/servo/string-cache) provides a global cache that can be created at compile time as well as at run time. Dynamic strings in the cache appear to be reference-counted so will be freed when they are no longer used, while `Ustr`s are never deleted. 

Creating a `string_cache::DefaultAtom` is much slower than creating a `Ustr`, especially in a multi-threaded context. On the other hand if you can just bake all your `Atom`s into your binary at compile-time this wouldn't be an issue. 

# Compared to string-interner
[string-interner](https://github.com/robbepop/string-interner) gives you individual `Interner` objects to work with rather than a global cache, which could be more flexible. It's faster to create than string-cache but still significantly slower than `Ustr`. 

# Speed
Ustrs are significantly faster to create than string-interner or string-cache. Creating 100,000 cycled copies of ~20,000 path strings of the form:
```
/cgi-bin/images/admin
/modules/templates/cache
/libraries/themes/wp-includes
...etc.
```

![raft bench](ustring_bench_raft.png)

# Testing
Note that tests must be run with RUST_TEST_THREADS=1 or some tests will fail due to concurrent tests filling the cache or segfaults caused by concurrently clearing the cache. Note that this cannot happen in user code if you don't call the hidden functions documented DO NOT CALL. If you do, well you were warned.

# Why?
It is common in certain types of applications to use strings as identifiers, but not really do any processing with them. To paraphrase from OIIO's ustring documentation...

Compared to standard strings, `Ustr`s have several advantages:

- Each individual `Ustr` is very small -- in fact, we guarantee that a `Ustr` is the same size and memory layout as an ordinary *u8.
- Storage is frugal, since there is only one allocated copy of each unique character sequence, throughout the lifetime of the program.
- Assignment from one `Ustr` to another is just copy of the pointer; no allocation, no character copying, no reference counting.
- Equality testing (do the strings contain the same characters) is a single operation, the comparison of the pointer.
- Memory allocation only occurs when a new `Ustr` is constructed from raw characters the FIRST time -- subsequent constructions of the same string just finds it in the canonial string set, but doesn't need to allocate new storage.  Destruction of a `Ustr` is trivial, there is no de-allocation because the canonical version stays in the set.  Also, therefore, no user code mistake can lead to memory leaks.

But there are some problems, too.  Canonical strings are never freed from the table.  So in some sense all the strings "leak", but they only leak one copy for each unique string that the program ever comes across. Creating a `Ustr` is slower than `String::from()` on a single thread, and performance will be worse if trying to create many `Ustr`s in tight loops from multiple threads due to lock contention for the global cache.

On the whole, `Ustr`s are a really great string representation
- if you tend to have (relatively) few unique strings, but many copies of those strings;
- if you tend to make the same strings over and over again, and if it's relatively rare that a single unique character sequence is used only once in the entire lifetime of the program; - if your most common string operations are assignment and equality testing and you want them to be as fast as possible;
- if you are doing relatively little character-by-character assembly of strings, string concatenation, or other "string manipulation" (other than equality testing).

`Ustr`s are not so hot:
- if your program tends to have very few copies of each character sequence over the entire lifetime of the program;
- if your program tends to generate a huge variety of unique strings over its lifetime, each of which is used only a short time and then discarded, never to be needed again;
- if you don't need to do a lot of string assignment or equality testing, but lots of more complex string manipulation.

## Safety and Compatibility
This crate has been tested (a little) on x86_64 ONLY. It will not compile on architectures where the pointer size is no 64 bits.

## Licence
BSD+ License

Copyright (c) 2019 Anders Langlands

Redistribution and use in source and binary forms, with or without modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright notice, this list of conditions and the following disclaimer in the documentation and/or other materials provided with the distribution.

Subject to the terms and conditions of this license, each copyright holder and contributor hereby grants to those receiving rights under this license a perpetual, worldwide, non-exclusive, no-charge, royalty-free, irrevocable (except for failure to satisfy the conditions of this license) patent license to make, have made, use, offer to sell, sell, import, and otherwise transfer this software, where such license applies only to those patent claims, already acquired or hereafter acquired, licensable by such copyright holder or contributor that are necessarily infringed by:

(a) their Contribution(s) (the licensed copyrights of copyright holders and non-copyrightable additions of contributors, in source or binary form) alone; or

(b) combination of their Contribution(s) with the work of authorship to which such Contribution(s) was added by such copyright holder or contributor, if, at the time the Contribution is added, such addition causes such combination to be necessarily infringed. The patent license shall not apply to any other combinations which include the Contribution.

Except as expressly stated above, no rights or licenses from any copyright holder or contributor is granted under this license, whether expressly, by implication, estoppel or otherwise.

DISCLAIMER

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDERS OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

Contains code ported from [OpenImageIO](https://github.com/OpenImageIO/oiio), BSD 3-clause licence.

Contains a copy of Max Woolf's [Big List of Naughty Strings](https://github.com/minimaxir/big-list-of-naughty-strings), MIT licence.

Contains some strings from [SecLists](https://github.com/danielmiessler/SecLists), MIT licence.