package main

import (
	"crypto"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rsa"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"math/big"
	"net/http"
	"strings"
	"sync"
	"time"
)

// A deliberately small JWT verifier (RS256 / ES256 only) so the provider needs no MIDGARD library
// and no JWT dependency. It validates signature, issuer, audience and time claims.

type Claims struct {
	Issuer   string
	Subject  string
	ClientID string
	Audience []string
	Expires  time.Time
	Raw      map[string]any
}

type Verifier struct {
	Issuers  []string
	Audience string
	JWKSURL  string
	Leeway   time.Duration
	HTTP     *http.Client

	mu     sync.Mutex
	keys   map[string]crypto.PublicKey
	loaded time.Time
}

type jwk struct {
	Kty, Crv, Kid, Alg, N, E, X, Y string
}

func b64(s string) ([]byte, error) { return base64.RawURLEncoding.DecodeString(s) }

func (v *Verifier) refresh() error {
	hc := v.HTTP
	if hc == nil {
		hc = &http.Client{Timeout: 5 * time.Second}
	}
	resp, err := hc.Get(v.JWKSURL)
	if err != nil {
		return fmt.Errorf("jwks: %w", err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != 200 {
		return fmt.Errorf("jwks: status %d", resp.StatusCode)
	}
	var set struct{ Keys []jwk }
	if err := json.NewDecoder(resp.Body).Decode(&set); err != nil {
		return err
	}
	keys := map[string]crypto.PublicKey{}
	for _, k := range set.Keys {
		switch k.Kty {
		case "RSA":
			n, e1 := b64(k.N)
			e, e2 := b64(k.E)
			if e1 != nil || e2 != nil {
				continue
			}
			keys[k.Kid] = &rsa.PublicKey{N: new(big.Int).SetBytes(n), E: int(new(big.Int).SetBytes(e).Int64())}
		case "EC":
			if k.Crv != "P-256" {
				continue
			}
			x, e1 := b64(k.X)
			y, e2 := b64(k.Y)
			if e1 != nil || e2 != nil {
				continue
			}
			keys[k.Kid] = &ecdsa.PublicKey{Curve: elliptic.P256(), X: new(big.Int).SetBytes(x), Y: new(big.Int).SetBytes(y)}
		}
	}
	v.keys, v.loaded = keys, time.Now()
	return nil
}

func (v *Verifier) key(kid string) (crypto.PublicKey, error) {
	v.mu.Lock()
	defer v.mu.Unlock()
	if k, ok := v.keys[kid]; ok {
		return k, nil
	}
	// Unknown kid: refetch (rotation), but not more than once per 10 seconds.
	if time.Since(v.loaded) > 10*time.Second {
		if err := v.refresh(); err != nil {
			return nil, err
		}
	}
	if k, ok := v.keys[kid]; ok {
		return k, nil
	}
	return nil, errors.New("unknown signing key")
}

func (v *Verifier) Verify(token string) (*Claims, error) {
	parts := strings.Split(token, ".")
	if len(parts) != 3 {
		return nil, errors.New("malformed token")
	}
	hb, err := b64(parts[0])
	if err != nil {
		return nil, errors.New("malformed header")
	}
	var h struct{ Alg, Kid string }
	if json.Unmarshal(hb, &h) != nil {
		return nil, errors.New("malformed header")
	}
	if h.Alg != "RS256" && h.Alg != "ES256" {
		return nil, errors.New("unsupported algorithm")
	}
	k, err := v.key(h.Kid)
	if err != nil {
		return nil, err
	}
	sig, err := b64(parts[2])
	if err != nil {
		return nil, errors.New("malformed signature")
	}
	digest := sha256.Sum256([]byte(parts[0] + "." + parts[1]))
	switch pk := k.(type) {
	case *rsa.PublicKey:
		if h.Alg != "RS256" || rsa.VerifyPKCS1v15(pk, crypto.SHA256, digest[:], sig) != nil {
			return nil, errors.New("bad signature")
		}
	case *ecdsa.PublicKey:
		if h.Alg != "ES256" || len(sig) != 64 ||
			!ecdsa.Verify(pk, digest[:], new(big.Int).SetBytes(sig[:32]), new(big.Int).SetBytes(sig[32:])) {
			return nil, errors.New("bad signature")
		}
	default:
		return nil, errors.New("bad key")
	}
	pb, err := b64(parts[1])
	if err != nil {
		return nil, errors.New("malformed payload")
	}
	var raw map[string]any
	if json.Unmarshal(pb, &raw) != nil {
		return nil, errors.New("malformed payload")
	}
	c := &Claims{Raw: raw}
	c.Issuer, _ = raw["iss"].(string)
	c.Subject, _ = raw["sub"].(string)
	c.ClientID, _ = raw["client_id"].(string)
	switch a := raw["aud"].(type) {
	case string:
		c.Audience = []string{a}
	case []any:
		for _, x := range a {
			if s, ok := x.(string); ok {
				c.Audience = append(c.Audience, s)
			}
		}
	}
	// ID tokens must never be accepted as access tokens (ZITADEL signs both with typ JWT).
	for _, k := range []string{"nonce", "at_hash", "amr", "auth_time"} {
		if _, ok := raw[k]; ok {
			return nil, errors.New("id token presented as access token")
		}
	}
	okIss := false
	for _, i := range v.Issuers {
		okIss = okIss || i == c.Issuer
	}
	if !okIss {
		return nil, errors.New("untrusted issuer")
	}
	okAud := false
	for _, a := range c.Audience {
		okAud = okAud || a == v.Audience
	}
	if !okAud {
		return nil, errors.New("wrong audience")
	}
	exp, ok := raw["exp"].(float64)
	if !ok {
		return nil, errors.New("missing exp")
	}
	c.Expires = time.Unix(int64(exp), 0)
	now := time.Now()
	if now.After(c.Expires.Add(v.Leeway)) {
		return nil, errors.New("token expired")
	}
	if nbf, ok := raw["nbf"].(float64); ok && now.Add(v.Leeway).Before(time.Unix(int64(nbf), 0)) {
		return nil, errors.New("token not yet valid")
	}
	return c, nil
}
