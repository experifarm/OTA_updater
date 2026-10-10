//! Suffix array construction with the Larsson–Sadakane `qsufsort` algorithm (as used by bsdiff).
//!
//! Prefix doubling: after the pass with step `h`, suffixes are sorted by their first `2h` bytes.
//! Unsorted groups are refined with a ternary-split quicksort keyed on the group number of the
//! suffix `h` positions later. `I[i] < 0` marks a run of `-I[i]` already-sorted entries that the next
//! pass skips. The original recursive `split` is replaced by an explicit job stack that preserves its
//! left → assign → right processing order (group renumbering relies on it).
//!
//! The result has `n + 1` entries; entry 0 is the empty suffix (`n`).

/// Maximum supported input length (indices are stored as `i32`).
pub const MAX_LEN: usize = i32::MAX as usize - 1;

/// Build the suffix array of `data`. Returns `sa` with `sa.len() == data.len() + 1`, `sa[0] == n`.
pub fn suffix_array(data: &[u8]) -> Vec<i32> {
    let n = data.len();
    assert!(n <= MAX_LEN, "input too large for i32 suffix array");
    let mut ia = vec![0i32; n + 1];
    let mut v = vec![0i32; n + 1];

    // Bucket sort by first byte. After this, buckets[c] is the index of the last slot used by
    // byte c, which is the initial group number of every suffix starting with c.
    let mut buckets = [0i32; 256];
    for &b in data {
        buckets[b as usize] += 1;
    }
    for i in 1..256 {
        buckets[i] += buckets[i - 1];
    }
    for i in (1..256).rev() {
        buckets[i] = buckets[i - 1];
    }
    buckets[0] = 0;
    for (i, &b) in data.iter().enumerate() {
        buckets[b as usize] += 1;
        ia[buckets[b as usize] as usize] = i as i32;
    }
    ia[0] = n as i32;
    for (i, &b) in data.iter().enumerate() {
        v[i] = buckets[b as usize];
    }
    v[n] = 0;
    for i in 1..256 {
        if buckets[i] == buckets[i - 1] + 1 {
            ia[buckets[i] as usize] = -1;
        }
    }
    ia[0] = -1;

    let total = (n + 1) as i32;
    let mut h = 1usize;
    while ia[0] != -total {
        let mut len = 0usize; // length of the current run of sorted entries
        let mut i = 0usize;
        while i < n + 1 {
            if ia[i] < 0 {
                let run = (-ia[i]) as usize;
                len += run;
                i += run;
            } else {
                if len != 0 {
                    ia[i - len] = -(len as i32);
                }
                let group_len = v[ia[i] as usize] as usize + 1 - i;
                split(&mut ia, &mut v, i, group_len, h);
                i += group_len;
                len = 0;
            }
        }
        if len != 0 {
            ia[i - len] = -(len as i32);
        }
        h += h;
    }

    // Every suffix now has a unique group number equal to its rank.
    for i in 0..=n {
        ia[v[i] as usize] = i as i32;
    }
    ia
}

enum Job {
    Split {
        start: usize,
        len: usize,
    },
    /// Renumber the equal-key middle partition `[jj, kk)` once everything left of it is done.
    Assign {
        jj: usize,
        kk: usize,
    },
}

fn split(ia: &mut [i32], v: &mut [i32], start: usize, len: usize, h: usize) {
    let mut stack = vec![Job::Split { start, len }];
    while let Some(job) = stack.pop() {
        match job {
            Job::Assign { jj, kk } => {
                for i in jj..kk {
                    v[ia[i] as usize] = (kk - 1) as i32;
                }
                if jj == kk - 1 {
                    ia[jj] = -1;
                }
            }
            Job::Split { start, len } if len < 16 => small_sort(ia, v, start, len, h),
            Job::Split { start, len } => {
                let key = |ia: &[i32], v: &[i32], i: usize| v[ia[i] as usize + h];
                let x = key(ia, v, start + len / 2);
                let (mut jj, mut kk) = (0usize, 0usize);
                for i in start..start + len {
                    let kv = key(ia, v, i);
                    if kv < x {
                        jj += 1;
                    }
                    if kv == x {
                        kk += 1;
                    }
                }
                jj += start;
                kk += jj;

                // Three-way partition: [start, jj) < x, [jj, kk) == x, [kk, start+len) > x.
                let (mut i, mut j, mut k) = (start, 0usize, 0usize);
                while i < jj {
                    let kv = key(ia, v, i);
                    if kv < x {
                        i += 1;
                    } else if kv == x {
                        ia.swap(i, jj + j);
                        j += 1;
                    } else {
                        ia.swap(i, kk + k);
                        k += 1;
                    }
                }
                while jj + j < kk {
                    if key(ia, v, jj + j) == x {
                        j += 1;
                    } else {
                        ia.swap(jj + j, kk + k);
                        k += 1;
                    }
                }

                // LIFO: pushed in reverse of the required execution order.
                if start + len > kk {
                    stack.push(Job::Split {
                        start: kk,
                        len: start + len - kk,
                    });
                }
                stack.push(Job::Assign { jj, kk });
                if jj > start {
                    stack.push(Job::Split { start, len: jj - start });
                }
            }
        }
    }
}

/// Selection-sort style refinement for small groups (from the original qsufsort).
fn small_sort(ia: &mut [i32], v: &mut [i32], start: usize, len: usize, h: usize) {
    let end = start + len;
    let mut k = start;
    while k < end {
        let mut j = 1usize;
        let mut x = v[ia[k] as usize + h];
        let mut i = 1usize;
        while k + i < end {
            let kv = v[ia[k + i] as usize + h];
            if kv < x {
                x = kv;
                j = 0;
            }
            if kv == x {
                ia.swap(k + j, k + i);
                j += 1;
            }
            i += 1;
        }
        for t in 0..j {
            v[ia[k + t] as usize] = (k + j - 1) as i32;
        }
        if j == 1 {
            ia[k] = -1;
        }
        k += j;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn naive(data: &[u8]) -> Vec<i32> {
        let mut sa: Vec<i32> = (0..=data.len() as i32).collect();
        sa.sort_by(|&a, &b| data[a as usize..].cmp(&data[b as usize..]));
        sa
    }

    #[test]
    fn small_cases() {
        for data in [
            &b""[..],
            b"a",
            b"aa",
            b"banana",
            b"mississippi",
            b"abracadabra",
            b"\x00\x00\x00\x01\x00",
            b"zyxwvutsrqponmlkjihgfedcba",
        ] {
            assert_eq!(suffix_array(data), naive(data), "{:?}", data);
        }
    }

    #[test]
    fn repetitive_and_large_groups() {
        let zeros = vec![0u8; 5000];
        assert_eq!(suffix_array(&zeros), naive(&zeros));
        let periodic: Vec<u8> = (0..6000).map(|i| b"abcab"[i % 5]).collect();
        assert_eq!(suffix_array(&periodic), naive(&periodic));
        let mut s = 7u32;
        let random: Vec<u8> = (0..20_000)
            .map(|_| {
                s = s.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                (s >> 16) as u8 % 4
            })
            .collect();
        assert_eq!(suffix_array(&random), naive(&random));
    }

    proptest! {
        #[test]
        fn matches_naive(data in proptest::collection::vec(0u8..6, 0..600)) {
            prop_assert_eq!(suffix_array(&data), naive(&data));
        }

        #[test]
        fn matches_naive_full_alphabet(data in proptest::collection::vec(any::<u8>(), 0..400)) {
            prop_assert_eq!(suffix_array(&data), naive(&data));
        }
    }
}
