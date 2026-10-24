// Command echo is the synthetic MIDGARD v0.2 fixture: a language-independent provider and client
// for the example.echo.submit capability. It imports no MIDGARD library.
package main

import (
	"context"
	"fmt"
	"net/http"
	"os"
	"os/signal"
	"syscall"
	"time"
)

func main() {
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	if len(os.Args) < 2 {
		fmt.Fprintln(os.Stderr, "usage: echo provider | client <cmd> | nats-init | healthcheck <url>")
		os.Exit(2)
	}
	var code int
	var err error
	switch os.Args[1] {
	case "provider":
		loadIDPEnv()
		err = runProvider(ctx)
	case "client":
		loadIDPEnv()
		code, err = runClient(ctx, os.Args[2:])
	case "nats-init":
		err = runNatsInit(ctx)
	case "healthcheck": // for distroless containers without curl: echo healthcheck <url>
		err = healthcheck(os.Args[2:])
	default:
		err, code = fmt.Errorf("unknown command %q", os.Args[1]), 2
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, "error:", err)
		if code == 0 {
			code = 1
		}
	}
	os.Exit(code)
}

func healthcheck(args []string) error {
	if len(args) != 1 {
		return fmt.Errorf("usage: echo healthcheck <url>")
	}
	c := http.Client{Timeout: 3 * time.Second}
	r, err := c.Get(args[0])
	if err != nil {
		return err
	}
	defer r.Body.Close()
	if r.StatusCode != 200 {
		return fmt.Errorf("status %d", r.StatusCode)
	}
	return nil
}
