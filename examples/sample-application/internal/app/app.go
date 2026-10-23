// Package app is a sample MIDGARD application facade. It never names a service or endpoint: it
// asks the platform to resolve the contract `platform.example`, then calls the implementation
// the platform bound to this application.
package app

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"strings"

	midgard "github.com/dejanvukadinovic/midgard/sdk/go"
	"github.com/dejanvukadinovic/midgard/sdk/go/contracts/platformexample"
)

const (
	Contract   = "platform.example"
	Constraint = ">=1.0.0 <2.0.0"
)

type App struct {
	Platform *midgard.Client
}

func (a *App) api(ctx context.Context, userToken, org string) (*platformexample.ClientWithResponses, *midgard.Resolution, error) {
	res, err := a.Platform.Resolve(ctx, Contract, Constraint)
	if err != nil {
		return nil, nil, err
	}
	editors := []platformexample.ClientOption{
		platformexample.WithRequestEditorFn(a.Platform.RequestEditor(Contract)),
		platformexample.WithRequestEditorFn(func(_ context.Context, r *http.Request) error {
			if userToken != "" {
				r.Header.Set("X-Delegated-User-Token", userToken)
			}
			if org != "" {
				r.Header.Set("X-Organization-Id", org)
			}
			return nil
		}),
	}
	c, err := platformexample.NewClientWithResponses(res.Endpoint, editors...)
	return c, res, err
}

// Echo calls the contract's echo operation.
func (a *App) Echo(ctx context.Context, message string) (map[string]any, error) {
	c, res, err := a.api(ctx, "", "")
	if err != nil {
		return nil, err
	}
	r, err := c.EchoWithResponse(ctx, platformexample.EchoJSONRequestBody{Message: message})
	if err != nil {
		return nil, err
	}
	if r.JSON200 == nil {
		return nil, statusError(r.StatusCode(), r.Body)
	}
	return map[string]any{"message": r.JSON200.Message, "application": r.JSON200.Application, "resolved_version": res.ResolvedVersion}, nil
}

// CreateNote stores a tenant-owned note for the delegated user's tenant.
func (a *App) CreateNote(ctx context.Context, userToken, org, body string) (map[string]any, error) {
	c, _, err := a.api(ctx, userToken, org)
	if err != nil {
		return nil, err
	}
	r, err := c.CreateNoteWithResponse(ctx, platformexample.CreateNoteJSONRequestBody{Body: body})
	if err != nil {
		return nil, err
	}
	if r.JSON201 == nil {
		return nil, statusError(r.StatusCode(), r.Body)
	}
	return map[string]any{"id": r.JSON201.Id.String(), "body": r.JSON201.Body}, nil
}

// ListNotes returns the notes of the delegated user's tenant.
func (a *App) ListNotes(ctx context.Context, userToken, org string) ([]map[string]any, error) {
	c, _, err := a.api(ctx, userToken, org)
	if err != nil {
		return nil, err
	}
	r, err := c.ListNotesWithResponse(ctx)
	if err != nil {
		return nil, err
	}
	if r.JSON200 == nil {
		return nil, statusError(r.StatusCode(), r.Body)
	}
	out := []map[string]any{}
	for _, n := range r.JSON200.Items {
		out = append(out, map[string]any{"id": n.Id.String(), "body": n.Body})
	}
	return out, nil
}

// StatusError carries an upstream HTTP status so callers can map denials distinctly.
type StatusError struct {
	Status int
	Body   string
}

func (e *StatusError) Error() string {
	return fmt.Sprintf("upstream returned %d: %s", e.Status, e.Body)
}

func statusError(status int, body []byte) error {
	return &StatusError{Status: status, Body: string(body)}
}

// Handler exposes the facade over HTTP. The browser's user token is forwarded as the delegated
// identity; tenant context is always derived by the platform from it.
func (a *App) Handler() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /healthz", func(w http.ResponseWriter, _ *http.Request) { w.WriteHeader(http.StatusOK) })
	mux.HandleFunc("GET /api/echo", func(w http.ResponseWriter, r *http.Request) {
		out, err := a.Echo(r.Context(), r.URL.Query().Get("message"))
		write(w, out, err)
	})
	mux.HandleFunc("GET /api/notes", func(w http.ResponseWriter, r *http.Request) {
		tok, ok := bearer(r)
		if !ok {
			http.Error(w, "user token required", http.StatusUnauthorized)
			return
		}
		out, err := a.ListNotes(r.Context(), tok, r.Header.Get("X-Organization-Id"))
		write(w, out, err)
	})
	mux.HandleFunc("POST /api/notes", func(w http.ResponseWriter, r *http.Request) {
		tok, ok := bearer(r)
		if !ok {
			http.Error(w, "user token required", http.StatusUnauthorized)
			return
		}
		var in struct {
			Body string `json:"body"`
		}
		if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 8192)).Decode(&in); err != nil {
			http.Error(w, "bad request", http.StatusBadRequest)
			return
		}
		out, err := a.CreateNote(r.Context(), tok, r.Header.Get("X-Organization-Id"), in.Body)
		write(w, out, err)
	})
	return mux
}

func bearer(r *http.Request) (string, bool) {
	h := r.Header.Get("Authorization")
	if !strings.HasPrefix(h, "Bearer ") {
		return "", false
	}
	return strings.TrimPrefix(h, "Bearer "), true
}

func write(w http.ResponseWriter, v any, err error) {
	if err != nil {
		status := http.StatusBadGateway
		if midgard.IsDenied(err) {
			status = http.StatusForbidden
		}
		if se, ok := err.(*StatusError); ok && (se.Status == 401 || se.Status == 403) {
			status = se.Status
		}
		http.Error(w, err.Error(), status)
		return
	}
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(v)
}
