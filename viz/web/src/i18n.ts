// All user-visible copy, in English and Chinese. `t(key, params)` interpolates `{name}` placeholders.

export type Lang = 'en' | 'zh';

type Dict = Record<string, string>;

const en: Dict = {
  title: 'OTA, explained',
  subtitle:
    'Watch one firmware update travel from a sender to many devices. Every packet, loss and repair below is computed by the real Rust engine running in your browser.',
  play: 'Play',
  pause: 'Pause',
  restart: 'Restart',
  prev: 'Previous moment',
  next: 'Next moment',
  speed: 'Speed',
  scale: 'Scale',
  scaleToy: 'Toy (readable text)',
  scaleReal: 'Real (1 MiB firmware)',
  scenario: 'Scenario',
  computing: 'Running the real engine…',
  customize: 'Customize the channel',
  rerun: 'Run again',
  happening: 'What is happening',
  legend: 'Legend',
  update: 'The update',
  seed: 'Seed',

  'preset.clean': 'Clean air',
  'preset.clean.d': 'No losses: watch the pipeline itself.',
  'preset.wifi': 'Noisy Wi-Fi',
  'preset.wifi.d': 'Random losses, plus corrupted, duplicated and reordered packets.',
  'preset.storm': 'Burst storm',
  'preset.storm.d': 'Losses arrive in bursts; interleaving spreads them across groups.',
  'preset.late': 'Late joiner',
  'preset.late.d': 'One device starts listening halfway through the broadcast.',
  'preset.attacker': 'Attacker',
  'preset.attacker.d': 'Forged shards and a fake update signed with the wrong key.',
  'preset.refused': 'Refusals',
  'preset.refused.d': 'Rollback, a different base image and a wrong public key are refused.',

  'band.old': 'old image v1.0.0',
  'band.new': 'new image v1.1.0',
  'band.diff': 'diff: copy + adjust',
  'band.extra': 'brand-new bytes',
  'band.patch': 'patch',
  'band.groups': '{groups} groups × ({k} data + {m} parity) shards',
  'band.sender': 'Sender',
  'band.attacker': 'Attacker',
  'band.round': 'Round {r}',

  'rx.asleep': 'Not listening yet',
  'rx.listening': 'Listening…',
  'rx.receiving': 'Receiving {done}/{total} groups',
  'rx.installed': 'Installed v{fw} ✓',
  'rx.rejected': 'Refused',
  'rx.failed': 'Failed',
  'rx.clean': 'clean channel',
  'rx.loss': 'loss {x}%',
  'rx.burst': 'bursts ×{x}',
  'rx.corrupt': 'corrupt {x}%',
  'rx.dup': 'dup {x}%',
  'rx.reorder': 'reorder {x}%',
  'rx.oldfw': 'runs v{v}',
  'rx.otherbase': 'other base image',
  'rx.otherkey': 'wrong public key',
  'rx.write': 'new image (slot B)',
  'rx.waiting': '{n} group(s) waiting for group {g} — bytes are applied in order',
  'air.label': 'UDP multicast · one transmission reaches every device',
  'rx.seal': 'manifest',

  'n.intro':
    'The sender compared both images: {records} regions could be copied from the old image, only {extra} bytes are brand new. The {newSize} image becomes a {patch} patch ({ratio} of the new image).',
  'n.introTiny':
    'The sender compared both images: {records} regions could be copied from the old image, only {extra} bytes are brand new. This toy image is so small that the patch ({patch}, with headers) is not smaller than the image — switch to the Real scale to see patches of a few percent.',
  'n.slice':
    'The patch is cut into {groups} groups of {k} data shards. Reed-Solomon adds {m} parity shards per group, so any {k} of the {n} shards rebuild the group.',
  'n.sign':
    'A manifest lists the SHA-256 fingerprint of every group and of the new image, and is signed with Ed25519. Devices trust only the publisher’s pinned public key.',
  'n.round1': 'Round 1: {packets} packets are broadcast over UDP multicast — one transmission reaches every device.',
  'n.roundN': 'Round {r}: the sender repeats everything, like a radio station, so late or unlucky devices can catch up.',
  'n.attacker': 'An attacker injects a fake update and forged shards with valid checksums.',
  'n.late': '{name} starts listening late; it will pick up what it missed in the next round.',
  'n.manifestOk': '{name}: manifest signature verified with the pinned public key ✓',
  'n.badSig': '{name}: a manifest with an invalid signature was ignored ✗',
  'n.lost': '{name}: packets are lost in the air — parity shards will cover them.',
  'n.crc': '{name}: a corrupted packet failed its CRC32 check and was dropped.',
  'n.rebuilt': '{name}: group {g} was missing {n} shard(s) — Reed-Solomon rebuilt them.',
  'n.hashBad': '{name}: group {g} did not match its signed fingerprint — forged data discarded, collecting it again.',
  'n.stream': '{name}: started writing the new image while packets are still arriving (streaming patch).',
  'n.installed': '{name}: the whole new image matches the signed hash — installed ✓',
  'n.rejected': '{name}: update refused — {reason}.',
  'n.failed': '{name}: update failed — {reason}.',

  'reason.rollback': 'it would be a rollback to an older or equal version',
  'reason.base': 'the device runs a different base image',
  'reason.timeout': 'no correctly signed manifest arrived (wrong key?)',
  'reason.ended': 'the broadcast ended before every group arrived',

  'leg.data': 'data shard',
  'leg.parity': 'parity shard',
  'leg.pad': 'padding (never sent)',
  'leg.manifest': 'signed manifest',
  'leg.attacker': 'attacker packet',
  'leg.lost': 'lost in the air',
  'leg.corrupt': 'corrupted → CRC ✗',
  'leg.rebuilt': 'rebuilt by Reed-Solomon',
  'leg.diff': 'written from old + diff',
  'leg.extra': 'written from new bytes',

  'card.session': 'session',
  'card.fw': 'firmware',
  'card.newsha': 'new image SHA-256',
  'card.sig': 'Ed25519 signature',
  'card.key': 'publisher key',
  'card.sizes': 'old {old} → new {new} · raw patch {raw} → {patch}{z}',
};

const zh: Dict = {
  title: 'OTA 升级，图解',
  subtitle: '看一次固件升级如何从一个发送端同时传到多台设备。下面的每个数据包、每次丢失和修复，都由浏览器里运行的真实 Rust 引擎计算得出。',
  play: '播放',
  pause: '暂停',
  restart: '重新开始',
  prev: '上一个关键时刻',
  next: '下一个关键时刻',
  speed: '速度',
  scale: '规模',
  scaleToy: '玩具（可读文本）',
  scaleReal: '真实（1 MiB 固件）',
  scenario: '场景',
  computing: '正在运行真实引擎…',
  customize: '自定义信道',
  rerun: '重新运行',
  happening: '正在发生什么',
  legend: '图例',
  update: '本次升级',
  seed: '随机种子',

  'preset.clean': '理想信道',
  'preset.clean.d': '没有丢包：专注看流程本身。',
  'preset.wifi': '嘈杂 Wi-Fi',
  'preset.wifi.d': '随机丢包，外加损坏、重复和乱序的数据包。',
  'preset.storm': '突发干扰',
  'preset.storm.d': '丢包成串出现；交织发送把它们分散到不同分组。',
  'preset.late': '中途加入',
  'preset.late.d': '一台设备在广播进行到一半时才开始监听。',
  'preset.attacker': '攻击者',
  'preset.attacker.d': '伪造的分片，以及用错误密钥签名的假升级包。',
  'preset.refused': '拒绝场景',
  'preset.refused.d': '回滚、基础镜像不同、公钥不对，都会被拒绝。',

  'band.old': '旧镜像 v1.0.0',
  'band.new': '新镜像 v1.1.0',
  'band.diff': '差分：复制 + 调整',
  'band.extra': '全新字节',
  'band.patch': '补丁',
  'band.groups': '{groups} 组 ×（{k} 数据 + {m} 校验）分片',
  'band.sender': '发送端',
  'band.attacker': '攻击者',
  'band.round': '第 {r} 轮',

  'rx.asleep': '尚未开始监听',
  'rx.listening': '监听中…',
  'rx.receiving': '接收中 {done}/{total} 组',
  'rx.installed': '已安装 v{fw} ✓',
  'rx.rejected': '已拒绝',
  'rx.failed': '失败',
  'rx.clean': '信道良好',
  'rx.loss': '丢包 {x}%',
  'rx.burst': '突发 ×{x}',
  'rx.corrupt': '损坏 {x}%',
  'rx.dup': '重复 {x}%',
  'rx.reorder': '乱序 {x}%',
  'rx.oldfw': '当前 v{v}',
  'rx.otherbase': '基础镜像不同',
  'rx.otherkey': '公钥不对',
  'rx.write': '新镜像（B 分区）',
  'rx.waiting': '{n} 组在等第 {g} 组——补丁必须按顺序应用',
  'air.label': 'UDP 组播 · 一次发送，所有设备同时收到',
  'rx.seal': '清单',

  'n.intro':
    '发送端比较了新旧两个镜像：{records} 段内容可以从旧镜像复制，只有 {extra} 字节是全新的。{newSize} 的新镜像变成了 {patch} 的补丁（新镜像的 {ratio}）。',
  'n.introTiny':
    '发送端比较了新旧两个镜像：{records} 段内容可以从旧镜像复制，只有 {extra} 字节是全新的。这个玩具镜像太小，加上头部后补丁（{patch}）并不比镜像小——切换到“真实”规模，可以看到只有百分之几大小的补丁。',
  'n.slice': '补丁被切成 {groups} 组，每组 {k} 个数据分片。里德-所罗门码为每组再加 {m} 个校验分片：{n} 片中任意收到 {k} 片就能还原整组。',
  'n.sign': '清单记录每组和新镜像的 SHA-256 指纹，并用 Ed25519 签名。设备只信任预置的发布者公钥。',
  'n.round1': '第 1 轮：通过 UDP 组播发出 {packets} 个数据包——一次发送，所有设备同时收到。',
  'n.roundN': '第 {r} 轮：发送端像电台一样把所有内容再播一遍，让晚到或运气不好的设备补齐。',
  'n.attacker': '攻击者注入了一个假升级包，以及校验和有效的伪造分片。',
  'n.late': '{name} 晚开始监听；错过的部分会在下一轮补上。',
  'n.manifestOk': '{name}：用预置公钥验证清单签名通过 ✓',
  'n.badSig': '{name}：签名无效的清单被忽略 ✗',
  'n.lost': '{name}：有数据包在空中丢失——校验分片会补上。',
  'n.crc': '{name}：一个损坏的数据包没通过 CRC32 校验，被丢弃。',
  'n.rebuilt': '{name}：第 {g} 组缺了 {n} 片——里德-所罗门码把它们算了回来。',
  'n.hashBad': '{name}：第 {g} 组与签名指纹不符——伪造数据被丢弃，重新收集。',
  'n.stream': '{name}：数据包还在到达，就已开始写入新镜像（流式还原）。',
  'n.installed': '{name}：整个新镜像与签名哈希一致——安装完成 ✓',
  'n.rejected': '{name}：拒绝升级——{reason}。',
  'n.failed': '{name}：升级失败——{reason}。',

  'reason.rollback': '这会回滚到更旧或相同的版本',
  'reason.base': '设备上的基础镜像不同',
  'reason.timeout': '没有收到正确签名的清单（公钥不对？）',
  'reason.ended': '广播结束前没有收齐所有分组',

  'leg.data': '数据分片',
  'leg.parity': '校验分片',
  'leg.pad': '填充（从不发送）',
  'leg.manifest': '已签名清单',
  'leg.attacker': '攻击者数据包',
  'leg.lost': '空中丢失',
  'leg.corrupt': '损坏 → CRC ✗',
  'leg.rebuilt': '由里德-所罗门码重建',
  'leg.diff': '由旧镜像 + 差分写入',
  'leg.extra': '由全新字节写入',

  'card.session': '会话',
  'card.fw': '固件',
  'card.newsha': '新镜像 SHA-256',
  'card.sig': 'Ed25519 签名',
  'card.key': '发布者公钥',
  'card.sizes': '旧 {old} → 新 {new} · 原始补丁 {raw} → {patch}{z}',

  'name.Car': '汽车',
  'name.Phone': '手机',
  'name.Watch': '手表',
  'name.Old car': '旧款汽车',
  'name.Other model': '其他型号',
  'name.Fake key': '错误公钥',
};

const dicts: Record<Lang, Dict> = { en, zh };

let lang: Lang = navigator.language?.toLowerCase().startsWith('zh') ? 'zh' : 'en';

export function getLang(): Lang {
  return lang;
}

export function setLang(l: Lang): void {
  lang = l;
  document.documentElement.lang = l === 'zh' ? 'zh-CN' : 'en';
}

export function t(key: string, params: Record<string, string | number> = {}): string {
  const s = dicts[lang][key] ?? dicts.en[key] ?? key;
  return s.replace(/\{(\w+)\}/g, (_, k: string) => (k in params ? String(params[k]) : `{${k}}`));
}

export function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(n < 10 * 1024 ? 1 : 0)} KiB`;
  return `${(n / 1024 / 1024).toFixed(2)} MiB`;
}

/** Localized display name of a device (names are engine identifiers; unknown names pass through). */
export function deviceName(name: string): string {
  const key = `name.${name}`;
  return dicts[lang][key] ?? name;
}

/** Map an engine reason string to localized text. */
export function reasonText(reason: string | null): string {
  if (!reason) return '';
  if (reason.includes('rollback')) return t('reason.rollback');
  if (reason.includes('different base image')) return t('reason.base');
  if (reason.includes('no valid manifest') || reason.includes('timed out')) return t('reason.timeout');
  if (reason.includes('broadcast ended')) return t('reason.ended');
  return reason;
}
