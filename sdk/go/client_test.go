package midgard

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"sync/atomic"
	"testing"
	"time"
)

type staticTokens struct{ audiences []string }

func (s *staticTokens) Token(_ context.Context, aud string) (string, error) {
	s.audiences = append(s.audiences, aud)
	return "tok-" + aud, nil
}

type clock struct{ t time.Time }

func (c *clock) now() time.Time { return c.t }

func problem(w http.ResponseWriter, status int, code string) {
	w.Header().Set("Content-Type", "application/problem+json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(map[string]any{"status": status, "code": code, "detail": "d"})
}

func newClient(t *testing.T, h http.HandlerFunc) (*Client, *clock, *staticTokens) {
	t.Helper()
	srv := httptest.NewServer(h)
	t.Cleanup(srv.Close)
	clk := &clock{t: time.Unix(1_700_000_000, 0)}
	ts := &staticTokens{}
	c, err := New(Config{RegistryURL: srv.URL, TokenSource: ts, Now: clk.now, Sleep: func(time.Duration) {}})
	if err != nil {
		t.Fatal(err)
	}
	return c, clk, ts
}

func okResolution(w http.ResponseWriter, endpoint string) {
	_ = json.NewEncoder(w).Encode(Resolution{Contract: "platform.example", ResolvedVersion: "1.0.0", Protocol: "http", Endpoint: endpoint, ExpiresIn: 300})
}

func TestResolveCachesUntilTTLAndUsesRegistryAudience(t *testing.T) {
	var hits int32
	c, clk, ts := newClient(t, func(w http.ResponseWriter, r *http.Request) {
		atomic.AddInt32(&hits, 1)
		if r.Header.Get("Authorization") != "Bearer tok-midgard-core" {
			problem(w, 401, "unauthenticated")
			return
		}
		okResolution(w, "http://svc:8080")
	})
	for i := 0; i < 3; i++ {
		res, err := c.Resolve(context.Background(), "platform.example", ">=1.0.0 <2.0.0")
		if err != nil || res.Endpoint != "http://svc:8080" {
			t.Fatalf("resolve: %v %v", res, err)
		}
	}
	if hits != 1 {
		t.Fatalf("expected 1 registry hit within TTL, got %d", hits)
	}
	clk.t = clk.t.Add(301 * time.Second)
	if _, err := c.Resolve(context.Background(), "platform.example", ">=1.0.0 <2.0.0"); err != nil || hits != 2 {
		t.Fatalf("expected refresh after TTL: hits=%d err=%v", hits, err)
	}
	for _, a := range ts.audiences {
		if a != "midgard-core" {
			t.Fatalf("registry calls must use the registry audience, got %q", a)
		}
	}
}

func TestStaleIfErrorIsBoundedAndRoutingOnly(t *testing.T) {
	var mode atomic.Value
	mode.Store("ok")
	c, clk, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) {
		switch mode.Load().(string) {
		case "ok":
			okResolution(w, "http://svc:8080")
		case "outage":
			problem(w, 502, "bad_gateway")
		case "denied":
			problem(w, 403, "forbidden")
		case "unhealthy":
			problem(w, 503, "no_healthy_deployment")
		}
	})
	ctx := context.Background()
	if _, err := c.Resolve(ctx, "platform.example", ">=1.0.0"); err != nil {
		t.Fatal(err)
	}
	// expired by 30s, registry down -> serve stale routing
	clk.t = clk.t.Add(330 * time.Second)
	mode.Store("outage")
	if res, err := c.Resolve(ctx, "platform.example", ">=1.0.0"); err != nil || res.Endpoint == "" {
		t.Fatalf("expected bounded stale answer, got %v %v", res, err)
	}
	// expired by more than MaxStale -> error
	clk.t = clk.t.Add(120 * time.Second)
	if _, err := c.Resolve(ctx, "platform.example", ">=1.0.0"); err == nil {
		t.Fatal("stale routing must not exceed MaxStale")
	}
	// authorization failures are never served stale
	mode.Store("ok")
	clk.t = clk.t.Add(1000 * time.Second)
	if _, err := c.Resolve(ctx, "platform.example", ">=1.0.0"); err != nil {
		t.Fatal(err)
	}
	clk.t = clk.t.Add(310 * time.Second)
	mode.Store("denied")
	_, err := c.Resolve(ctx, "platform.example", ">=1.0.0")
	if !IsDenied(err) {
		t.Fatalf("expected denial, got %v", err)
	}
	// an explicit "no healthy deployment" is an answer, not an outage
	mode.Store("unhealthy")
	if _, err := c.Resolve(ctx, "platform.example", ">=1.0.0"); err == nil {
		t.Fatal("must not serve stale when the registry says nothing is healthy")
	}
}

func TestTransientErrorsAreRetriedButDenialsAreNot(t *testing.T) {
	var hits int32
	c, _, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) {
		n := atomic.AddInt32(&hits, 1)
		if n < 3 {
			problem(w, 502, "bad_gateway")
			return
		}
		okResolution(w, "http://svc:8080")
	})
	if _, err := c.Resolve(context.Background(), "platform.example", ">=1.0.0"); err != nil || hits != 3 {
		t.Fatalf("expected success after 2 retries, hits=%d err=%v", hits, err)
	}
	hits = 0
	c2, _, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) {
		atomic.AddInt32(&hits, 1)
		problem(w, 403, "forbidden")
	})
	if _, err := c2.Resolve(context.Background(), "platform.example", ">=1.0.0"); !IsDenied(err) || hits != 1 {
		t.Fatalf("403 must not be retried: hits=%d err=%v", hits, err)
	}
}

func TestCallUsesContractAudienceAndDoesNotReplayNonIdempotent(t *testing.T) {
	var capHits int32
	capability := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		atomic.AddInt32(&capHits, 1)
		if r.Header.Get("Authorization") != "Bearer tok-platform.example" {
			problem(w, 401, "unauthenticated")
			return
		}
		hj, _ := w.(http.Hijacker) // simulate a connection dropped mid-flight
		conn, _, _ := hj.Hijack()
		conn.Close()
	}))
	defer capability.Close()
	c, _, ts := newClient(t, func(w http.ResponseWriter, r *http.Request) { okResolution(w, capability.URL) })

	_, err := c.Call(context.Background(), "platform.example", ">=1.0.0", http.MethodPost, "/v1/notes", []byte(`{"body":"x"}`), DoOptions{})
	if err == nil || capHits != 1 {
		t.Fatalf("non-idempotent call must not be replayed: hits=%d err=%v", capHits, err)
	}
	capHits = 0
	_, err = c.Call(context.Background(), "platform.example", ">=1.0.0", http.MethodGet, "/v1/ping", nil, DoOptions{Idempotent: true})
	if err == nil || capHits != 2 {
		t.Fatalf("idempotent call may retry once: hits=%d err=%v", capHits, err)
	}
	found := false
	for _, a := range ts.audiences {
		if a == "platform.example" {
			found = true
		}
	}
	if !found {
		t.Fatalf("capability calls need a token for the contract audience, got %v", ts.audiences)
	}
}

func TestResolveTenantAndMalformedResponses(t *testing.T) {
	c, _, _ := newClient(t, func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/v0/tenants/resolve":
			var b map[string]string
			_ = json.NewDecoder(r.Body).Decode(&b)
			if b["user_token"] == "" {
				problem(w, 403, "forbidden")
				return
			}
			_ = json.NewEncoder(w).Encode(AuthContext{TenantID: "t1", OrganizationID: b["organization_id"]})
		default:
			_, _ = w.Write([]byte(`{"endpoint":""}`))
		}
	})
	got, err := c.ResolveTenant(context.Background(), "user-token", "orgB")
	if err != nil || got.TenantID != "t1" || got.OrganizationID != "orgB" {
		t.Fatalf("%v %v", got, err)
	}
	if _, err := c.ResolveTenant(context.Background(), "", ""); !IsDenied(err) {
		t.Fatalf("expected denial, got %v", err)
	}
	if _, err := c.Resolve(context.Background(), "platform.example", ">=1.0.0"); err == nil {
		t.Fatal("malformed resolution must be rejected")
	}
}

func TestClientCredentialsCachesPerAudience(t *testing.T) {
	var hits int32
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		atomic.AddInt32(&hits, 1)
		u, p, _ := r.BasicAuth()
		if u != "cid" || p != "sec" || r.FormValue("grant_type") != "client_credentials" {
			w.WriteHeader(401)
			return
		}
		_ = json.NewEncoder(w).Encode(map[string]any{"access_token": "t-" + r.FormValue("audience"), "expires_in": 300})
	}))
	defer srv.Close()
	cc := &ClientCredentials{TokenURL: srv.URL, ClientID: "cid", ClientSecret: "sec"}
	for i := 0; i < 3; i++ {
		if tok, err := cc.Token(context.Background(), "a"); err != nil || tok != "t-a" {
			t.Fatal(tok, err)
		}
	}
	if tok, _ := cc.Token(context.Background(), "b"); tok != "t-b" || hits != 2 {
		t.Fatalf("tokens must be cached per audience, hits=%d", hits)
	}
	bad := &ClientCredentials{TokenURL: srv.URL, ClientID: "cid", ClientSecret: "wrong"}
	if _, err := bad.Token(context.Background(), "a"); err == nil {
		t.Fatal("expected failure")
	}
}
