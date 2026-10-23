package midgard

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"
)

// TokenSource yields access tokens for a given audience. Discovery tokens for the registry and
// calls to a capability use different audiences and must never be interchanged.
type TokenSource interface {
	Token(ctx context.Context, audience string) (string, error)
}

// ClientCredentials is an OAuth2 client-credentials token source with per-audience caching.
// Prefer private-key JWT client authentication in production; this uses client_secret_basic.
type ClientCredentials struct {
	TokenURL     string
	ClientID     string
	ClientSecret string
	HTTPClient   *http.Client
	// Skew is how long before expiry a cached token is refreshed (default 30s).
	Skew time.Duration

	mu    sync.Mutex
	cache map[string]cachedToken
}

type cachedToken struct {
	token   string
	expires time.Time
}

func (c *ClientCredentials) Token(ctx context.Context, audience string) (string, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	skew := c.Skew
	if skew == 0 {
		skew = 30 * time.Second
	}
	if t, ok := c.cache[audience]; ok && time.Now().Add(skew).Before(t.expires) {
		return t.token, nil
	}
	form := url.Values{"grant_type": {"client_credentials"}}
	if audience != "" {
		form.Set("audience", audience)
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, c.TokenURL, strings.NewReader(form.Encode()))
	if err != nil {
		return "", err
	}
	req.Header.Set("Content-Type", "application/x-www-form-urlencoded")
	req.SetBasicAuth(c.ClientID, c.ClientSecret)
	hc := c.HTTPClient
	if hc == nil {
		hc = http.DefaultClient
	}
	resp, err := hc.Do(req)
	if err != nil {
		return "", fmt.Errorf("midgard: token request: %w", err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return "", fmt.Errorf("midgard: token endpoint returned %d", resp.StatusCode)
	}
	var body struct {
		AccessToken string `json:"access_token"`
		ExpiresIn   int    `json:"expires_in"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&body); err != nil || body.AccessToken == "" {
		return "", fmt.Errorf("midgard: malformed token response")
	}
	if c.cache == nil {
		c.cache = map[string]cachedToken{}
	}
	c.cache[audience] = cachedToken{token: body.AccessToken, expires: time.Now().Add(time.Duration(body.ExpiresIn) * time.Second)}
	return body.AccessToken, nil
}
