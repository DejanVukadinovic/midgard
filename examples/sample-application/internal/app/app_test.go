package app

import (
	"net/http"
	"net/http/httptest"
	"testing"

	midgard "github.com/dejanvukadinovic/midgard/sdk/go"
)

func TestNotesRequireUserToken(t *testing.T) {
	c, _ := midgard.New(midgard.Config{RegistryURL: "http://127.0.0.1:1", TokenSource: &midgard.ClientCredentials{}})
	h := (&App{Platform: c}).Handler()
	for _, m := range []string{http.MethodGet, http.MethodPost} {
		rec := httptest.NewRecorder()
		h.ServeHTTP(rec, httptest.NewRequest(m, "/api/notes", nil))
		if rec.Code != http.StatusUnauthorized {
			t.Fatalf("%s /api/notes without a user token = %d, want 401", m, rec.Code)
		}
	}
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, httptest.NewRequest(http.MethodGet, "/healthz", nil))
	if rec.Code != http.StatusOK {
		t.Fatalf("healthz = %d", rec.Code)
	}
}
