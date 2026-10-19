// Tauri's base64-wrapped Minisign format, verified against the original public key.
const fs = require('node:fs');
const crypto = require('node:crypto');
const path = require('node:path');
function verify(bytes, encodedSignature, encodedPublicKey) {
  const pubLines = Buffer.from(encodedPublicKey.trim(), 'base64').toString('utf8').trim().split(/\r?\n/);
  const sigLines = Buffer.from(encodedSignature.trim(), 'base64').toString('utf8').trim().split(/\r?\n/);
  const pub = Buffer.from(pubLines[1] || '', 'base64');
  const sig = Buffer.from(sigLines[1] || '', 'base64');
  const global = Buffer.from(sigLines[3] || '', 'base64');
  if (pub.length !== 42 || sig.length !== 74 || global.length !== 64 ||
      pub.subarray(0, 2).toString() !== 'Ed' || sig.subarray(0, 2).toString() !== 'ED' ||
      !sig.subarray(2, 10).equals(pub.subarray(2, 10)) || !sigLines[2]?.startsWith('trusted comment: ')) {
    throw Error('Signature does not match the original Sfery key or format');
  }
  const key = crypto.createPublicKey({key: Buffer.concat([Buffer.from('302a300506032b6570032100', 'hex'), pub.subarray(10)]), format: 'der', type: 'spki'});
  if (!crypto.verify(null, crypto.createHash('blake2b512').update(bytes).digest(), key, sig.subarray(10)) ||
      !crypto.verify(null, Buffer.concat([sig.subarray(10), Buffer.from(sigLines[2].slice(17))]), key, global)) {
    throw Error('Invalid updater signature');
  }
  return sigLines[2].slice(17);
}
module.exports = {verify};
if (require.main === module) {
  const file = process.argv[2];
  if (!file) throw Error('Usage: node scripts/verify-signature.cjs <installer>');
  const config = JSON.parse(fs.readFileSync(path.join(__dirname, '../src-tauri/tauri.production.conf.json')));
  verify(fs.readFileSync(file), fs.readFileSync(file + '.sig', 'utf8'), config.plugins.updater.pubkey);
  console.log('Updater signature verified against the original Sfery public key.');
}
