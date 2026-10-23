package main

import (
	"os"
	"strings"
)

func env(k, def string) string {
	if v := os.Getenv(k); v != "" {
		return v
	}
	return def
}

// secret reads K_FILE (preferred) or K.
func secret(k string) string {
	if f := os.Getenv(k + "_FILE"); f != "" {
		if b, err := os.ReadFile(f); err == nil {
			return strings.TrimSpace(string(b))
		}
	}
	return os.Getenv(k)
}

type providerConfig struct {
	bind, issuerBase, jwksURL, audience              string
	issuers                                          []string
	coreURL, coreAudience, tokenURL                  string
	clientID, clientSecret                           string
	publicEndpoint, slug, environment                string
	natsURL, natsUser, natsPassword, natsDestination string
	natsBinding                                      string
	debug                                            bool
}

func loadProviderConfig() providerConfig {
	iss := strings.TrimRight(env("ECHO_ISSUER", ""), "/")
	c := providerConfig{
		bind:           env("ECHO_BIND_ADDR", "0.0.0.0:8080"),
		issuerBase:     iss,
		issuers:        []string{iss, iss + "/"},
		jwksURL:        env("ECHO_JWKS_URL", iss+"/oauth/v2/keys"),
		audience:       env("ECHO_AUDIENCE", "example.echo"),
		coreURL:        strings.TrimRight(env("ECHO_CORE_URL", ""), "/"),
		coreAudience:   env("ECHO_CORE_AUDIENCE", "midgard-core"),
		tokenURL:       env("ECHO_TOKEN_URL", iss+"/oauth/v2/token"),
		clientID:       env("ECHO_CLIENT_ID", ""),
		clientSecret:   secret("ECHO_CLIENT_SECRET"),
		publicEndpoint: env("ECHO_PUBLIC_ENDPOINT", ""),
		slug:           env("ECHO_SLUG", "example-echo"),
		environment:    env("ECHO_ENVIRONMENT", "local"),
		natsURL:        env("ECHO_NATS_URL", ""),
		natsUser:       env("ECHO_NATS_USER", "provider"),
		natsPassword:   secret("ECHO_NATS_PASSWORD"),
		natsBinding:    env("ECHO_NATS_BINDING", "default-mq"),
		debug:          env("ECHO_DEBUG", "") == "true",
	}
	return c
}
