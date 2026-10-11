//! Toy-scale "firmware": short readable text, so every byte, diff and shard can be shown on screen.

/// Old and new toy images (UTF-8 text).
pub fn images() -> (Vec<u8>, Vec<u8>) {
    let old = "\
TOY-FW v1.0.0
[boot] clock=48MHz sensors=temp,humidity
[motor] speed=1200rpm ramp=2s
[valve] open=30% close_after=10min
[net] wifi=garden retry=3 timeout=5s
[log] level=info keep=7d
[ui] led=green blink=slow
[safety] max_temp=60C shutdown=on
[crc] 7f3a91c2
";
    let new = "\
TOY-FW v1.1.0
[boot] clock=48MHz sensors=temp,humidity,light
[motor] speed=1500rpm ramp=2s
[valve] open=30% close_after=10min
[net] wifi=garden retry=5 timeout=5s
[sec] signed_updates=only
[log] level=info keep=7d
[ui] led=blue blink=slow
[safety] max_temp=60C shutdown=on
[crc] 1b8e04d7
";
    (old.as_bytes().to_vec(), new.as_bytes().to_vec())
}
