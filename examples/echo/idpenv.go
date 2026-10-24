package main

import (
	"bufio"
	"os"
	"path/filepath"
	"strings"
)

// loadIDPEnv adopts the identity-provider facts published by the ZITADEL bootstrap job
// (<dir>/idp.env and <dir>/secrets.env, default dir /idp). When the files exist they take precedence over
// the static development defaults in the environment; without them this is a no-op (dev-IdP mode).
func loadIDPEnv() {
	dir := env("ECHO_IDP_DIR", "/idp")
	m := map[string]string{}
	for _, f := range []string{"idp.env", "secrets.env"} {
		fh, err := os.Open(filepath.Join(dir, f))
		if err != nil {
			continue
		}
		sc := bufio.NewScanner(fh)
		for sc.Scan() {
			k, v, ok := strings.Cut(strings.TrimSpace(sc.Text()), "=")
			if !ok || strings.HasPrefix(k, "#") {
				continue
			}
			m[k] = strings.Trim(v, `'"`)
		}
		fh.Close()
	}
	if len(m) == 0 {
		return
	}
	set := func(k, v string) {
		if v != "" {
			os.Setenv(k, v)
		}
	}
	set("ECHO_ISSUER", m["IDP_ISSUER"])
	set("ECHO_TOKEN_URL", m["IDP_TOKEN_URL"])
	set("ECHO_JWKS_URL", m["IDP_JWKS_URL"])
	set("ECHO_CORE_AUDIENCE", m["PLATFORM_AUDIENCE"])
	set("ECHO_AUDIENCE", m["ECHO_AUDIENCE"])
	set("ECHO_PROVIDER_AUDIENCE", m["ECHO_AUDIENCE"])
	if cid := os.Getenv("ECHO_CLIENT_ID"); cid != "" {
		key := "SECRET_" + strings.ToUpper(strings.ReplaceAll(cid, "-", "_"))
		os.Unsetenv("ECHO_CLIENT_SECRET_FILE")
		set("ECHO_CLIENT_SECRET", m[key])
	}
}
