package main

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"
)

// tokenSource: OAuth2 client credentials with caching (stdlib only).
type tokenSource struct {
	url, id, secret string
	hc              *http.Client
	mu              sync.Mutex
	cache           map[string]struct {
		tok string
		exp time.Time
	}
}

func (t *tokenSource) Token(ctx context.Context, audience string) (string, error) {
	t.mu.Lock()
	defer t.mu.Unlock()
	if c, ok := t.cache[audience]; ok && time.Now().Add(30*time.Second).Before(c.exp) {
		return c.tok, nil
	}
	form := url.Values{"grant_type": {"client_credentials"}}
	if audience != "" {
		if strings.Trim(audience, "0123456789") == "" {
			form.Set("scope", "openid urn:zitadel:iam:org:project:id:"+audience+":aud") // ZITADEL project id
		} else {
			form.Set("audience", audience)
		}
	}
	req, _ := http.NewRequestWithContext(ctx, "POST", t.url, strings.NewReader(form.Encode()))
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	req.SetBasicAuth(t.id, t.secret)
	resp, err := t.hc.Do(req)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()
	if resp.StatusCode != 200 {
		return "", fmt.Errorf("token endpoint returned %d", resp.StatusCode)
	}
	var b struct {
		AccessToken string `json:"access_token"`
		ExpiresIn   int    `json:"expires_in"`
	}
	if json.NewDecoder(resp.Body).Decode(&b) != nil || b.AccessToken == "" {
		return "", fmt.Errorf("malformed token response")
	}
	if t.cache == nil {
		t.cache = map[string]struct {
			tok string
			exp time.Time
		}{}
	}
	t.cache[audience] = struct {
		tok string
		exp time.Time
	}{b.AccessToken, time.Now().Add(time.Duration(b.ExpiresIn) * time.Second)}
	return b.AccessToken, nil
}

type coreClient struct {
	base     string
	audience string
	ts       *tokenSource
	hc       *http.Client
}

func (c *coreClient) do(ctx context.Context, method, path string, in, out any) (int, error) {
	tok, err := c.ts.Token(ctx, c.audience)
	if err != nil {
		return 0, err
	}
	var body io.Reader
	if in != nil {
		b, _ := json.Marshal(in)
		body = bytes.NewReader(b)
	}
	req, _ := http.NewRequestWithContext(ctx, method, c.base+path, body)
	req.Header.Set("Authorization", "Bearer "+tok)
	if in != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	resp, err := c.hc.Do(req)
	if err != nil {
		return 0, err
	}
	defer resp.Body.Close()
	raw, _ := io.ReadAll(io.LimitReader(resp.Body, 4<<20))
	if out != nil && resp.StatusCode < 300 {
		if err := json.Unmarshal(raw, out); err != nil {
			return resp.StatusCode, err
		}
	}
	return resp.StatusCode, nil
}
