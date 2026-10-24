package main

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"net/http"
	"os"
	"strings"
	"time"

	"github.com/nats-io/nats.go"
	"github.com/nats-io/nats.go/jetstream"
)

// The synthetic client application: resolves example.echo.submit through MIDGARD Core, then
// talks to the provider directly (HTTP) or through the broker (queue). It uses no MIDGARD library.
//
// Exit codes: 0 ok, 1 error, 3 denied (authorization failure at Core, provider or broker).

const exitDenied = 3

type resolved struct {
	Digest     string `json:"digest"`
	Version    string `json:"resolved_version"`
	Interfaces []struct {
		ID            string `json:"id"`
		Protocol      string `json:"protocol"`
		Location      string `json:"location"`
		Destination   string `json:"destination"`
		BrokerBinding string `json:"broker_binding"`
		Specification struct {
			Summary struct {
				Method string `json:"method"`
				Path   string `json:"path"`
			} `json:"summary"`
		} `json:"specification"`
	} `json:"interfaces"`
}

type clientCfg struct {
	app, coreURL, coreAudience, tokenURL, id, secret  string
	providerAudience, natsURL, natsUser, natsPassword string
	environment                                       string
}

func loadClientCfg() clientCfg {
	iss := strings.TrimRight(env("ECHO_ISSUER", ""), "/")
	return clientCfg{
		app:              env("ECHO_APP", "obroklik"),
		coreURL:          strings.TrimRight(env("ECHO_CORE_URL", ""), "/"),
		coreAudience:     env("ECHO_CORE_AUDIENCE", "midgard-core"),
		tokenURL:         env("ECHO_TOKEN_URL", iss+"/oauth/v2/token"),
		id:               env("ECHO_CLIENT_ID", ""),
		secret:           secret("ECHO_CLIENT_SECRET"),
		providerAudience: env("ECHO_PROVIDER_AUDIENCE", "example.echo"),
		natsURL:          env("ECHO_NATS_URL", ""),
		natsUser:         env("ECHO_NATS_USER", "client-obroklik"),
		natsPassword:     secret("ECHO_NATS_PASSWORD"),
		environment:      env("ECHO_ENVIRONMENT", "local"),
	}
}

type denied struct{ msg string }

func (d denied) Error() string { return "denied: " + d.msg }

func (c clientCfg) resolve(ctx context.Context, ts *tokenSource, hc *http.Client) (*resolved, error) {
	core := &coreClient{base: c.coreURL, audience: c.coreAudience, ts: ts, hc: hc}
	var r resolved
	status, err := core.do(ctx, "POST", "/v0/contracts/resolve", map[string]any{
		"contract": contractName, "version_constraint": "=" + contractVersion, "environment": c.environment,
	}, &r)
	if err != nil {
		return nil, err
	}
	switch status {
	case 200:
		return &r, nil
	case 401, 403:
		return nil, denied{fmt.Sprintf("resolution refused (HTTP %d)", status)}
	default:
		return nil, fmt.Errorf("resolution failed (HTTP %d)", status)
	}
}

func runClient(ctx context.Context, args []string) (int, error) {
	if len(args) == 0 {
		return 1, errors.New("usage: echo client resolve|submit-http|submit-queue|publish-raw [flags]")
	}
	fs := flag.NewFlagSet(args[0], flag.ContinueOnError)
	id := fs.String("id", "", "submission id")
	value := fs.String("value", "hello", "value")
	subject := fs.String("subject", "", "publish-raw: exact subject")
	userToken := fs.String("user-token", "", "submit-http: delegated user token")
	org := fs.String("organization", "", "submit-http: organization id")
	ctype := fs.String("content-type", "application/json", "submit-http: content type")
	body := fs.String("body", "", "raw body (submit-http, publish-raw)")
	bearer := fs.String("bearer", "", "submit-http: override the bearer token (negative tests)")
	if err := fs.Parse(args[1:]); err != nil {
		return 1, err
	}
	cfg := loadClientCfg()
	hc := &http.Client{Timeout: 10 * time.Second}
	ts := &tokenSource{url: cfg.tokenURL, id: cfg.id, secret: cfg.secret, hc: hc}

	switch args[0] {
	case "resolve":
		r, err := cfg.resolve(ctx, ts, hc)
		if err != nil {
			return codeFor(err), err
		}
		return 0, json.NewEncoder(os.Stdout).Encode(r)

	case "submit-http":
		r, err := cfg.resolve(ctx, ts, hc)
		if err != nil {
			return codeFor(err), err
		}
		for _, i := range r.Interfaces {
			if i.ID != "rest" {
				continue
			}
			tok := *bearer
			if tok == "" {
				if tok, err = ts.Token(ctx, cfg.providerAudience); err != nil {
					return 1, err
				}
			}
			payload := []byte(*body)
			if *body == "" {
				payload, _ = json.Marshal(map[string]string{"id": *id, "value": *value})
			}
			req, _ := http.NewRequestWithContext(ctx, i.Specification.Summary.Method, strings.TrimRight(i.Location, "/")+i.Specification.Summary.Path, bytes.NewReader(payload))
			req.Header.Set("Authorization", "Bearer "+tok)
			req.Header.Set("Content-Type", *ctype)
			if *userToken != "" {
				req.Header.Set("X-Delegated-User-Token", *userToken)
				req.Header.Set("X-Organization-Id", *org)
			}
			resp, err := hc.Do(req)
			if err != nil {
				return 1, err
			}
			defer resp.Body.Close()
			raw, _ := io.ReadAll(resp.Body)
			fmt.Printf("%d %s\n", resp.StatusCode, strings.TrimSpace(string(raw)))
			switch {
			case resp.StatusCode == 202:
				return 0, nil
			case resp.StatusCode == 401 || resp.StatusCode == 403:
				return exitDenied, denied{fmt.Sprintf("provider answered %d", resp.StatusCode)}
			default:
				return 1, fmt.Errorf("provider answered %d", resp.StatusCode)
			}
		}
		return exitDenied, denied{"no usable http interface was resolved"}

	case "submit-queue":
		r, err := cfg.resolve(ctx, ts, hc)
		if err != nil {
			return codeFor(err), err
		}
		for _, i := range r.Interfaces {
			if i.ID != "queue" {
				continue
			}
			// The deployment convention (enforced by broker ACLs): publish to <destination>.<application>.
			payload, _ := json.Marshal(map[string]string{"id": *id, "value": *value})
			return publish(ctx, cfg, i.Destination+"."+cfg.app, *id, payload)
		}
		return exitDenied, denied{"no usable queue interface was resolved"}

	case "publish-raw": // negative testing: bypass resolution, talk to the broker directly
		payload := []byte(*body)
		if *body == "" {
			payload = []byte(`{"id":"` + *id + `","value":"raw"}`)
		}
		return publish(ctx, cfg, *subject, *id, payload)
	}
	return 1, fmt.Errorf("unknown command %q", args[0])
}

func codeFor(err error) int {
	var d denied
	if errors.As(err, &d) {
		return exitDenied
	}
	return 1
}

func publish(ctx context.Context, cfg clientCfg, subject, id string, payload []byte) (int, error) {
	nc, err := nats.Connect(cfg.natsURL, nats.UserInfo(cfg.natsUser, cfg.natsPassword), nats.Timeout(5*time.Second))
	if err != nil {
		return exitDenied, denied{"broker refused the connection: " + err.Error()}
	}
	defer nc.Close()
	js, err := jetstream.New(nc)
	if err != nil {
		return 1, err
	}
	m := nats.NewMsg(subject)
	m.Data = payload
	m.Header.Set("Nats-Msg-Id", id)
	m.Header.Set("Midgard-Correlation-Id", "corr-"+id)
	pctx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()
	ack, err := js.PublishMsg(pctx, m)
	if err != nil {
		// A permissions violation surfaces as a missing JetStream acknowledgment.
		return exitDenied, denied{"publish not acknowledged: " + err.Error()}
	}
	fmt.Printf("published %s seq=%d duplicate=%v\n", subject, ack.Sequence, ack.Duplicate)
	return 0, nil
}
