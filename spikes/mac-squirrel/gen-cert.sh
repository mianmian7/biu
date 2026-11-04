#!/usr/bin/env bash
# 生成临时自签名代码签名证书（CN=Biu Test Signing），只用于 CI spike，job 结束即丢弃。
set -euo pipefail
out=${1:?out dir}
mkdir -p "$out"; cd "$out"
cat > cert.cnf <<'CNF'
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = Biu Test Signing
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
subjectKeyIdentifier = hash
CNF
openssl req -x509 -newkey rsa:2048 -nodes -days 30 -config cert.cnf -keyout key.pem -out biu-test.pem
openssl x509 -in biu-test.pem -outform der -out biu-test.cer
pass=$(openssl rand -hex 16)
openssl pkcs12 -export -legacy -inkey key.pem -in biu-test.pem -out biu-test.p12 -passout "pass:$pass" 2>/dev/null \
  || openssl pkcs12 -export -inkey key.pem -in biu-test.pem -out biu-test.p12 -passout "pass:$pass"
rm key.pem
# electron-builder 用 CSC_LINK 自建临时 keychain 时 set-key-partition-list 解锁失败（macos-26 runner），
# 所以自己建 keychain 导入，只给 electron-builder 传 CSC_NAME。
kc="$out/build.keychain-db"; kpw=$(openssl rand -hex 16)
echo "::add-mask::$pass"; echo "::add-mask::$kpw"
security create-keychain -p "$kpw" "$kc"
security set-keychain-settings -lut 21600 "$kc"
security unlock-keychain -p "$kpw" "$kc"
security import biu-test.p12 -k "$kc" -P "$pass" -T /usr/bin/codesign -T /usr/bin/productsign
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$kpw" "$kc" > /dev/null
security list-keychains -d user -s "$kc" $(security list-keychains -d user | tr -d '"')
rm biu-test.p12
{ echo "CSC_NAME=Biu Test Signing"; echo "CSC_IDENTITY_AUTO_DISCOVERY=true"; } >> "$GITHUB_ENV"
