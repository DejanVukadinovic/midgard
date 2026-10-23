// Package midgard is the Go SDK for MIDGARD: client-side contract resolution with a bounded
// routing cache, delegated tenant resolution, and authenticated invocation helpers.
package midgard

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math/rand"
	"net/http"
	"strings"
	"sync"
	"time"
)

// Config configures a Client.
type Config struct {
	// RegistryURL is the base URL of MIDGARD core (e.g. https://platform.example).
	RegistryURL string
	// RegistryAudience is the audience of tokens presented to the registry (default "midgard-core").
	RegistryAudience string
	TokenSource      TokenSource
	// Environment selects the deployment environment (default: the registry's default).
	Environment string
	// Audience maps a contract to the audience of tokens presented to its implementations.
	// Default: the contract name itself.
	Audience   func(contract string) string
	HTTPClient *http.Client
	// MaxStale bounds how long a cached *routing* answer may be reused when the registry is
	// unreachable (default 60s). Authorization failures are never served stale.
	MaxStale time.Duration
	// Retries is the number of retries for transient resolver failures (default 2).
	Retries int
	// Now and Sleep are injectable for tests.
	Now   func() time.Time
	Sleep func(time.Duration)
}

// Resolution is the registry's answer for a contract.
type Resolution struct {
	Contract        string   `json:"contract"`
	ResolvedVersion string   `json:"resolved_version"`
	Protocol        string   `json:"protocol"`
	Endpoint        string   `json:"endpoint"`
	Permissions     []string `json:"permissions"`
	ExpiresIn       int      `json:"expires_in_seconds"`
}

// AuthContext is the tenant context the platform derived for a delegated user.
type AuthContext struct {
	SubjectID      string   `json:"subject_id"`
	ApplicationID  string   `json:"application_id"`
	OrganizationID string   `json:"organization_id"`
	TenantID       string   `json:"tenant_id"`
	Roles          []string `json:"roles"`
	ActorType      string   `json:"actor_type"`
}

type cacheEntry struct {
	res     Resolution
	fresh   time.Time // until when the answer is fresh
	fetched time.Time
}

// Client resolves contracts. Its cache is private to the Client (one credential / authorization
// context) and must not be shared across credentials.
type Client struct {
	cfg   Config
	mu    sync.Mutex
	cache map[string]cacheEntry
}

func New(cfg Config) (*Client, error) {
	if cfg.RegistryURL == "" || cfg.TokenSource == nil {
		return nil, errors.New("midgard: RegistryURL and TokenSource are required")
	}
	cfg.RegistryURL = strings.TrimRight(cfg.RegistryURL, "/")
	if cfg.RegistryAudience == "" {
		cfg.RegistryAudience = "midgard-core"
	}
	if cfg.Audience == nil {
		cfg.Audience = func(c string) string { return c }
	}
	if cfg.HTTPClient == nil {
		cfg.HTTPClient = &http.Client{Timeout: 10 * time.Second}
	}
	if cfg.MaxStale == 0 {
		cfg.MaxStale = 60 * time.Second
	}
	if cfg.Retries == 0 {
		cfg.Retries = 2
	}
	if cfg.Now == nil {
		cfg.Now = time.Now
	}
	if cfg.Sleep == nil {
		cfg.Sleep = time.Sleep
	}
	return &Client{cfg: cfg, cache: map[string]cacheEntry{}}, nil
}

// Resolve returns the endpoint for `contract` satisfying `constraint` (e.g. ">=1.0.0 <2.0.0").
// The application never names a service or endpoint; the registry decides via grants and bindings.
func (c *Client) Resolve(ctx context.Context, contract, constraint string) (*Resolution, error) {
	key := contract + "|" + constraint + "|" + c.cfg.Environment
	now := c.cfg.Now()
	c.mu.Lock()
	e, ok := c.cache[key]
	c.mu.Unlock()
	if ok && now.Before(e.fresh) {
		r := e.res
		return &r, nil
	}

	res, err := c.fetch(ctx, contract, constraint)
	if err == nil {
		ttl := time.Duration(res.ExpiresIn) * time.Second
		c.mu.Lock()
		c.cache[key] = cacheEntry{res: *res, fresh: c.cfg.Now().Add(ttl), fetched: c.cfg.Now()}
		c.mu.Unlock()
		return res, nil
	}
	// Bounded stale-if-error: only for registry unavailability, never for authorization or
	// "no healthy deployment" answers, and never beyond MaxStale after the entry expired.
	if ok && isRegistryOutage(err) && now.Before(e.fresh.Add(c.cfg.MaxStale)) {
		r := e.res
		return &r, nil
	}
	return nil, err
}

func isRegistryOutage(err error) bool {
	var p *Problem
	if errors.As(err, &p) {
		return (p.Status == http.StatusBadGateway || p.Status == http.StatusGatewayTimeout || p.Status == http.StatusServiceUnavailable) && p.Code != "no_healthy_deployment"
	}
	return true // transport errors
}

func transient(err error) bool {
	var p *Problem
	if errors.As(err, &p) {
		return (p.Status == http.StatusBadGateway || p.Status == http.StatusGatewayTimeout || (p.Status == http.StatusServiceUnavailable && p.Code != "no_healthy_deployment"))
	}
	return !errors.Is(err, context.Canceled)
}

func (c *Client) fetch(ctx context.Context, contract, constraint string) (*Resolution, error) {
	body := map[string]string{"contract": contract, "version_constraint": constraint}
	if c.cfg.Environment != "" {
		body["environment"] = c.cfg.Environment
	}
	var out Resolution
	var err error
	for attempt := 0; ; attempt++ {
		err = c.postJSON(ctx, "/v0/contracts/resolve", body, &out)
		if err == nil || attempt >= c.cfg.Retries || !transient(err) {
			break
		}
		// Exponential backoff with full jitter, bounded.
		backoff := time.Duration(100*(1<<attempt)) * time.Millisecond
		c.cfg.Sleep(time.Duration(rand.Int63n(int64(backoff) + 1)))
	}
	if err != nil {
		return nil, err
	}
	if out.Endpoint == "" || out.ExpiresIn <= 0 {
		return nil, fmt.Errorf("midgard: malformed resolution response")
	}
	return &out, nil
}

// ResolveTenant exchanges a delegated user's access token for a platform tenant context. The
// organization is only a *selection*; the platform verifies it against the user's grants.
func (c *Client) ResolveTenant(ctx context.Context, userToken, organizationID string) (*AuthContext, error) {
	body := map[string]string{"user_token": userToken}
	if organizationID != "" {
		body["organization_id"] = organizationID
	}
	var out AuthContext
	if err := c.postJSON(ctx, "/v0/tenants/resolve", body, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (c *Client) postJSON(ctx context.Context, path string, in, out any) error {
	tok, err := c.cfg.TokenSource.Token(ctx, c.cfg.RegistryAudience)
	if err != nil {
		return err
	}
	buf, _ := json.Marshal(in)
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, c.cfg.RegistryURL+path, bytes.NewReader(buf))
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Authorization", "Bearer "+tok)
	resp, err := c.cfg.HTTPClient.Do(req)
	if err != nil {
		return fmt.Errorf("midgard: registry request: %w", err)
	}
	defer resp.Body.Close()
	data, _ := io.ReadAll(io.LimitReader(resp.Body, 1<<20))
	if resp.StatusCode != http.StatusOK {
		return parseProblem(resp.StatusCode, data)
	}
	return json.Unmarshal(data, out)
}

// RequestEditor returns a function (compatible with generated clients' WithRequestEditorFn)
// that attaches a workload token whose audience is the contract's implementation.
func (c *Client) RequestEditor(contract string) func(ctx context.Context, req *http.Request) error {
	return func(ctx context.Context, req *http.Request) error {
		tok, err := c.cfg.TokenSource.Token(ctx, c.cfg.Audience(contract))
		if err != nil {
			return err
		}
		req.Header.Set("Authorization", "Bearer "+tok)
		return nil
	}
}

// DoOptions control Call.
type DoOptions struct {
	// Idempotent permits one transparent retry after a transport failure. Non-idempotent calls
	// (the default) are never replayed by the SDK.
	Idempotent bool
	Headers    map[string]string
}

// Call resolves the contract and performs one authenticated request against its implementation.
func (c *Client) Call(ctx context.Context, contract, constraint, method, path string, body []byte, opt DoOptions) (*http.Response, error) {
	res, err := c.Resolve(ctx, contract, constraint)
	if err != nil {
		return nil, err
	}
	attempts := 1
	if opt.Idempotent {
		attempts = 2
	}
	var lastErr error
	for i := 0; i < attempts; i++ {
		req, err := http.NewRequestWithContext(ctx, method, strings.TrimRight(res.Endpoint, "/")+path, bytes.NewReader(body))
		if err != nil {
			return nil, err
		}
		if len(body) > 0 {
			req.Header.Set("Content-Type", "application/json")
		}
		for k, v := range opt.Headers {
			req.Header.Set(k, v)
		}
		if err := c.RequestEditor(contract)(ctx, req); err != nil {
			return nil, err
		}
		resp, err := c.cfg.HTTPClient.Do(req)
		if err == nil {
			return resp, nil
		}
		lastErr = err
	}
	return nil, lastErr
}
