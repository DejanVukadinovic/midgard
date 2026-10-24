// sample-application: a minimal MIDGARD application facade.
//
//	sample-application serve                    run the HTTP facade
//	sample-application check [--user-token T] [--org O]   resolve + invoke once (used by acceptance tests)
package main

import (
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"log"
	"net/http"
	"os"
	"strings"
	"time"

	"github.com/dejanvukadinovic/midgard/examples/sample-application/internal/app"
	midgard "github.com/dejanvukadinovic/midgard/sdk/go"
)

func env(name, def string) string {
	if v := os.Getenv(name); v != "" {
		return v
	}
	return def
}

func secret(name string) string {
	if p := os.Getenv(name + "_FILE"); p != "" {
		b, err := os.ReadFile(p)
		if err != nil {
			log.Fatalf("read %s_FILE: %v", name, err)
		}
		return strings.TrimSpace(string(b))
	}
	return os.Getenv(name)
}

func build() *app.App {
	registry := env("MIDGARD_REGISTRY_URL", "http://localhost:8080")
	// MIDGARD_AUDIENCES="contract=idp-audience,..." maps contract names to the identity provider's
	// audience (ZITADEL project ids). Unmapped contracts use the contract name.
	audiences := map[string]string{}
	for _, kv := range strings.Split(os.Getenv("MIDGARD_AUDIENCES"), ",") {
		if k, v, ok := strings.Cut(strings.TrimSpace(kv), "="); ok {
			audiences[k] = v
		}
	}
	c, err := midgard.New(midgard.Config{
		RegistryURL:      registry,
		RegistryAudience: env("MIDGARD_REGISTRY_AUDIENCE", "midgard-core"),
		Environment:      env("MIDGARD_ENVIRONMENT", "local"),
		TokenSource: &midgard.ClientCredentials{
			TokenURL:     os.Getenv("MIDGARD_TOKEN_URL"),
			ClientID:     os.Getenv("MIDGARD_CLIENT_ID"),
			ClientSecret: secret("MIDGARD_CLIENT_SECRET"),
			Audiences:    audiences,
		},
	})
	if err != nil {
		log.Fatal(err)
	}
	return &app.App{Platform: c}
}

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintln(os.Stderr, "usage: sample-application serve|check")
		os.Exit(2)
	}
	switch os.Args[1] {
	case "serve":
		addr := env("SAMPLE_APP_ADDR", ":8081")
		srv := &http.Server{Addr: addr, Handler: build().Handler(), ReadHeaderTimeout: 5 * time.Second}
		log.Printf("sample-application listening on %s", addr)
		log.Fatal(srv.ListenAndServe())
	case "check":
		fs := flag.NewFlagSet("check", flag.ExitOnError)
		userToken := fs.String("user-token", "", "end-user access token (exercises the tenant-owned notes path)")
		org := fs.String("org", "", "organization selection (verified by the platform)")
		note := fs.String("note", "", "text of the note to create (default: generated)")
		_ = fs.Parse(os.Args[2:])
		a := build()
		ctx, cancel := context.WithTimeout(context.Background(), 20*time.Second)
		defer cancel()
		out := map[string]any{}
		echo, err := a.Echo(ctx, "hello from "+env("MIDGARD_CLIENT_ID", "app"))
		if err != nil {
			fail(err)
		}
		out["echo"] = echo
		if *userToken != "" {
			note, err := a.CreateNote(ctx, *userToken, *org, noteText(*note))
			if err != nil {
				fail(err)
			}
			notes, err := a.ListNotes(ctx, *userToken, *org)
			if err != nil {
				fail(err)
			}
			out["created"], out["notes"] = note, notes
		}
		_ = json.NewEncoder(os.Stdout).Encode(out)
	default:
		fmt.Fprintln(os.Stderr, "unknown command")
		os.Exit(2)
	}
}

// fail prints the error and exits 3 for authorization denials, 1 otherwise.
func fail(err error) {
	fmt.Fprintln(os.Stderr, "error:", err)
	var se *app.StatusError
	if midgard.IsDenied(err) || (errors.As(err, &se) && (se.Status == 401 || se.Status == 403)) {
		os.Exit(3)
	}
	os.Exit(1)
}

func noteText(given string) string {
	if given != "" {
		return given
	}
	return "note from " + env("MIDGARD_CLIENT_ID", "app")
}
