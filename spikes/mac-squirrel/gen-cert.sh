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
echo "::add-mask::$pass"
{ echo "CSC_LINK=$out/biu-test.p12"; echo "CSC_KEY_PASSWORD=$pass"; echo "CSC_NAME=Biu Test Signing"; } >> "$GITHUB_ENV"
