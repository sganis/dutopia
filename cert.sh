#!/bin/bash
# Generate self-signed certificate for local development on macOS

# Configuration
DOMAIN="localhost"
DAYS=365
CERT_FILE="cert.pem"
KEY_FILE="key.pem"

# Generate private key and certificate in one command
openssl req -x509 -newkey rsa:4096 -nodes \
  -keyout "$KEY_FILE" \
  -out "$CERT_FILE" \
  -days "$DAYS" \
  -subj "/C=US/ST=State/L=City/O=Organization/CN=$DOMAIN" \
  -addext "subjectAltName=DNS:$DOMAIN,DNS:*.localhost,IP:127.0.0.1"

echo "✅ Certificate generated successfully!"
echo ""
echo "Files created:"
echo "  Certificate: $CERT_FILE"
echo "  Private Key: $KEY_FILE"
echo ""
echo "Usage with your server:"
echo "  ./duapi data.csv --tls-cert $CERT_FILE --tls-key $KEY_FILE"
echo ""
echo "⚠️  Browser Warning:"
echo "Your browser will show a security warning because this is self-signed."
echo "This is normal for development. Click 'Advanced' and proceed."
echo ""
