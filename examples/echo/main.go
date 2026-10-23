// Command echo is the synthetic MIDGARD v0.2 fixture: a language-independent provider and client
// for the example.echo.submit capability. It imports no MIDGARD library.
package main

import (
	"context"
	"fmt"
	"os"
	"os/signal"
	"syscall"
)

func main() {
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	if len(os.Args) < 2 {
		fmt.Fprintln(os.Stderr, "usage: echo provider | client <cmd> | nats-init")
		os.Exit(2)
	}
	var code int
	var err error
	switch os.Args[1] {
	case "provider":
		err = runProvider(ctx)
	case "client":
		code, err = runClient(ctx, os.Args[2:])
	case "nats-init":
		err = runNatsInit(ctx)
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
