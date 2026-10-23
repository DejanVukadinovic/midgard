package main

import (
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

func b(v []byte) string { return base64.RawURLEncoding.EncodeToString(v) }

type fixture struct {
	key *ecdsa.PrivateKey
	srv *httptest.Server
}

func newFixture(t *testing.T) *fixture {
	k, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	f := &fixture{key: k}
	f.srv = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		_ = json.NewEncoder(w).Encode(map[string]any{"keys": []map[string]string{{
			"kty": "EC", "crv": "P-256", "kid": "k1", "alg": "ES256",
			"x": b(k.X.FillBytes(make([]byte, 32))), "y": b(k.Y.FillBytes(make([]byte, 32))),
		}}})
	}))
	t.Cleanup(f.srv.Close)
	return f
}

func (f *fixture) sign(alg string, claims map[string]any) string {
	h, _ := json.Marshal(map[string]string{"alg": alg, "kid": "k1"})
	c, _ := json.Marshal(claims)
	in := b(h) + "." + b(c)
	d := sha256.Sum256([]byte(in))
	r, s, _ := ecdsa.Sign(rand.Reader, f.key, d[:])
	sig := append(r.FillBytes(make([]byte, 32)), s.FillBytes(make([]byte, 32))...)
	return in + "." + b(sig)
}

func claims(mod func(map[string]any)) map[string]any {
	c := map[string]any{"iss": "https://idp", "aud": []string{"example.echo"}, "sub": "s", "client_id": "app", "exp": time.Now().Add(time.Minute).Unix()}
	if mod != nil {
		mod(c)
	}
	return c
}

func TestVerifier(t *testing.T) {
	f := newFixture(t)
	v := &Verifier{Issuers: []string{"https://idp"}, Audience: "example.echo", JWKSURL: f.srv.URL, Leeway: time.Second}
	good := f.sign("ES256", claims(nil))
	if c, err := v.Verify(good); err != nil || c.ClientID != "app" {
		t.Fatalf("valid token rejected: %v", err)
	}
	cases := map[string]string{
		"wrong audience": f.sign("ES256", claims(func(c map[string]any) { c["aud"] = "other" })),
		"wrong issuer":   f.sign("ES256", claims(func(c map[string]any) { c["iss"] = "https://evil" })),
		"expired":        f.sign("ES256", claims(func(c map[string]any) { c["exp"] = time.Now().Add(-time.Hour).Unix() })),
		"alg none":       strings.Replace(good, b([]byte(`{"alg":"ES256","kid":"k1"}`)), b([]byte(`{"alg":"none","kid":"k1"}`)), 1),
		"tampered":       good[:len(good)-2] + "AA",
		"garbage":        "not.a.jwt",
	}
	for name, tok := range cases {
		if _, err := v.Verify(tok); err == nil {
			t.Errorf("%s: token accepted", name)
		}
	}
}
