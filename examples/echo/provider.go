package main

import (
	"context"
	"encoding/json"
	"errors"
	"log/slog"
	"net/http"
	"os"
	"strings"
	"sync"
	"time"

	"github.com/nats-io/nats.go"
	"github.com/nats-io/nats.go/jetstream"
)

const (
	contractName    = "example.echo.submit"
	contractVersion = "1.0.0"
	streamName      = "EXAMPLE"
	consumerName    = "provider-submit"
	submitSubject   = "example.echo.submit"      // logical destination; clients publish to <destination>.<application>
	deadSubject     = "example.echo.dead.submit" // poison messages end up here
	maxDeliver      = 5
)

// Provider is a synthetic, MIDGARD-SDK-free implementation of example.echo.submit v1.0.0: an
// HTTP interface (OpenAPI) and a durable-queue interface (AsyncAPI) with the same "accepted for
// processing" semantics. It enforces authorization on every invocation by asking MIDGARD Core
// (/v0/authorize) as its approved deployment identity; the resolved endpoint grants nothing.
type Provider struct {
	verifier *Verifier
	core     *coreClient
	log      *slog.Logger

	mu        sync.Mutex
	processed map[string]Submission // by id: idempotent handling
	order     []string
	dead      []string // reasons for isolated messages (debug visibility)
}

type Submission struct {
	ID          string `json:"id"`
	Value       string `json:"value"`
	Via         string `json:"via"`
	Application string `json:"application"`
	Tenant      string `json:"tenant,omitempty"`
	Deliveries  int    `json:"deliveries"`
}

type authzResult struct {
	Application string `json:"application"`
	Tenant      *struct {
		TenantID string `json:"tenant_id"`
	} `json:"tenant"`
}

func (p *Provider) authorize(ctx context.Context, req map[string]any) (*authzResult, int) {
	req["contract"] = contractName
	var out authzResult
	status, err := p.core.do(ctx, "POST", "/v0/authorize", req, &out)
	if err != nil {
		p.log.Error("core authorize failed", "err", err)
		return nil, http.StatusServiceUnavailable
	}
	if status != 200 {
		return nil, status
	}
	return &out, 200
}

// record stores a submission once; redeliveries / retries are no-ops (idempotent consumer).
func (p *Provider) record(s Submission) (first bool) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if old, ok := p.processed[s.ID]; ok {
		old.Deliveries++
		p.processed[s.ID] = old
		return false
	}
	s.Deliveries = 1
	p.processed[s.ID] = s
	p.order = append(p.order, s.ID)
	return true
}

type submitReq struct {
	ID    string `json:"id"`
	Value string `json:"value"`
}

func (r submitReq) valid() bool {
	return r.ID != "" && len(r.ID) <= 128 && r.Value != "" && len(r.Value) <= 4096
}

func problem(w http.ResponseWriter, status int, code string) {
	w.Header().Set("Content-Type", "application/problem+json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(map[string]any{"status": status, "code": code})
}

// HTTP interface `rest` -> operation submitEcho (POST /submit, 202 Accepted).
func (p *Provider) handleSubmit(w http.ResponseWriter, r *http.Request) {
	tok := strings.TrimPrefix(r.Header.Get("Authorization"), "Bearer ")
	if tok == "" || tok == r.Header.Get("Authorization") {
		problem(w, http.StatusUnauthorized, "missing_token")
		return
	}
	claims, err := p.verifier.Verify(tok)
	if err != nil || claims.ClientID == "" {
		problem(w, http.StatusUnauthorized, "invalid_token")
		return
	}
	if ct := strings.TrimSpace(strings.Split(r.Header.Get("Content-Type"), ";")[0]); ct != "application/json" {
		problem(w, http.StatusUnsupportedMediaType, "unsupported_media_type")
		return
	}
	// Authorization first: nothing about the payload is processed for unauthorized callers.
	areq := map[string]any{"client_id": claims.ClientID, "interface": "rest"}
	if ut := r.Header.Get("X-Delegated-User-Token"); ut != "" {
		areq["user_token"] = ut
		if org := r.Header.Get("X-Organization-Id"); org != "" {
			areq["organization_id"] = org
		}
	}
	az, status := p.authorize(r.Context(), areq)
	if az == nil {
		if status == http.StatusForbidden || status == http.StatusUnauthorized {
			problem(w, http.StatusForbidden, "not_authorized")
		} else {
			problem(w, http.StatusServiceUnavailable, "authorization_unavailable")
		}
		return
	}
	var in submitReq
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 64<<10)).Decode(&in); err != nil || !in.valid() {
		problem(w, http.StatusBadRequest, "invalid_payload")
		return
	}
	s := Submission{ID: in.ID, Value: in.Value, Via: "rest", Application: az.Application}
	if az.Tenant != nil {
		s.Tenant = az.Tenant.TenantID
	}
	p.record(s)
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusAccepted) // accepted for processing, not completed
	_ = json.NewEncoder(w).Encode(map[string]any{"id": in.ID, "status": "accepted"})
}

func (p *Provider) handleDebug(w http.ResponseWriter, r *http.Request) {
	p.mu.Lock()
	defer p.mu.Unlock()
	out := make([]Submission, 0, len(p.order))
	for _, id := range p.order {
		out = append(out, p.processed[id])
	}
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(map[string]any{"items": out, "dead": p.dead})
}

func (p *Provider) Routes(debug bool) http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("POST /submit", p.handleSubmit)
	mux.HandleFunc("GET /health/ready", func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(200) })
	if debug {
		mux.HandleFunc("GET /debug/processed", p.handleDebug)
	}
	return mux
}

// consume runs the MQ interface `queue` -> operation submitEcho. The calling application is
// derived from the broker-authenticated subject (the broker only lets an application publish to
// <destination>.<its slug>), never from message headers or payload.
func (p *Provider) consume(ctx context.Context, js jetstream.JetStream) error {
	cons, err := js.Consumer(ctx, streamName, consumerName)
	if err != nil {
		return err
	}
	for ctx.Err() == nil {
		batch, err := cons.Fetch(1, jetstream.FetchMaxWait(2*time.Second))
		if err != nil {
			time.Sleep(500 * time.Millisecond)
			continue
		}
		for m := range batch.Messages() {
			p.handleMessage(ctx, js, m)
		}
	}
	return ctx.Err()
}

func (p *Provider) handleMessage(ctx context.Context, js jetstream.JetStream, m jetstream.Msg) {
	md, _ := m.Metadata()
	attempt := uint64(1)
	if md != nil {
		attempt = md.NumDelivered
	}
	err := p.process(ctx, m)
	switch {
	case err == nil:
		_ = m.DoubleAck(ctx)
	case attempt >= maxDeliver || errors.Is(err, errPermanent):
		// Poison / unauthorized: isolate, then stop redelivery.
		dm := nats.NewMsg(deadSubject)
		dm.Data = m.Data()
		dm.Header.Set("Midgard-Dead-Reason", err.Error())
		dm.Header.Set("Midgard-Original-Subject", m.Subject())
		dm.Header.Set("Nats-Msg-Id", "dead-"+m.Headers().Get("Nats-Msg-Id"))
		if _, perr := js.PublishMsg(ctx, dm); perr != nil {
			p.log.Error("dead-letter publish failed; leaving message for redelivery", "err", perr)
			_ = m.Nak()
			return
		}
		p.log.Warn("message isolated", "reason", err, "subject", m.Subject())
		p.mu.Lock()
		p.dead = append(p.dead, err.Error())
		p.mu.Unlock()
		_ = m.Term()
	default:
		_ = m.NakWithDelay(time.Second)
	}
}

var errPermanent = errors.New("permanent")

func (p *Provider) process(ctx context.Context, m jetstream.Msg) error {
	app := strings.TrimPrefix(m.Subject(), submitSubject+".")
	if app == "" || app == m.Subject() || strings.Contains(app, ".") {
		return errors.Join(errPermanent, errors.New("subject carries no application identity"))
	}
	az, status := p.authorize(ctx, map[string]any{"application": app, "interface": "queue"})
	if az == nil {
		if status == http.StatusForbidden || status == http.StatusUnauthorized || status == http.StatusNotFound {
			return errors.Join(errPermanent, errors.New("sender not authorized for this interface"))
		}
		return errors.New("authorization temporarily unavailable") // retry
	}
	var in submitReq
	if err := json.Unmarshal(m.Data(), &in); err != nil || !in.valid() {
		return errors.Join(errPermanent, errors.New("invalid payload"))
	}
	p.record(Submission{ID: in.ID, Value: in.Value, Via: "queue", Application: az.Application})
	return nil
}

func runProvider(ctx context.Context) error {
	cfg := loadProviderConfig()
	log := slog.New(slog.NewJSONHandler(os.Stderr, nil))
	hc := &http.Client{Timeout: 10 * time.Second}
	p := &Provider{
		verifier: &Verifier{Issuers: cfg.issuers, Audience: cfg.audience, JWKSURL: cfg.jwksURL, Leeway: 30 * time.Second},
		core: &coreClient{base: cfg.coreURL, audience: cfg.coreAudience, hc: hc,
			ts: &tokenSource{url: cfg.tokenURL, id: cfg.clientID, secret: cfg.clientSecret, hc: hc}},
		log:       log,
		processed: map[string]Submission{},
	}
	srv := &http.Server{Addr: cfg.bind, Handler: p.Routes(cfg.debug), ReadHeaderTimeout: 5 * time.Second}
	go func() {
		if err := srv.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
			log.Error("http server", "err", err)
			os.Exit(1)
		}
	}()
	go p.registerLoop(ctx, cfg)
	if cfg.natsURL != "" {
		nc, err := nats.Connect(cfg.natsURL, nats.UserInfo(cfg.natsUser, cfg.natsPassword), nats.MaxReconnects(-1), nats.RetryOnFailedConnect(true))
		if err != nil {
			return err
		}
		defer nc.Drain()
		js, err := jetstream.New(nc)
		if err != nil {
			return err
		}
		go func() {
			for ctx.Err() == nil {
				if err := p.consume(ctx, js); err != nil && ctx.Err() == nil {
					log.Warn("consumer stopped; retrying", "err", err)
					time.Sleep(2 * time.Second)
				}
			}
		}()
	}
	<-ctx.Done()
	shutdown, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	return srv.Shutdown(shutdown)
}
