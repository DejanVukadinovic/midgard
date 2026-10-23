package main

import (
	"context"
	"strconv"
	"time"
)

// registerLoop registers (and keeps re-asserting) this provider's interfaces with Core through its
// approved deployment identity. It reads the contract digest from the published contract, so the
// provider claims exactly what the registry holds.
func (p *Provider) registerLoop(ctx context.Context, cfg providerConfig) {
	for ctx.Err() == nil {
		if err := p.register(ctx, cfg); err != nil {
			p.log.Warn("registration failed; retrying", "err", err)
			select {
			case <-ctx.Done():
				return
			case <-time.After(3 * time.Second):
			}
			continue
		}
		p.log.Info("registered with core", "slug", cfg.slug)
		return
	}
}

func (p *Provider) register(ctx context.Context, cfg providerConfig) error {
	var c struct {
		Digest string `json:"digest"`
	}
	status, err := p.core.do(ctx, "GET", "/v0/contracts/"+contractName+"/versions/"+contractVersion, nil, &c)
	if err != nil {
		return err
	}
	if status != 200 {
		return &statusError{"contract not readable/published yet", status}
	}
	ifaces := []map[string]any{{"id": "rest", "location": cfg.publicEndpoint}}
	if cfg.natsURL != "" {
		ifaces = append(ifaces, map[string]any{"id": "queue", "destination": submitSubject, "broker_binding": cfg.natsBinding})
	}
	body := map[string]any{
		"slug": cfg.slug, "release_version": "0.2.0",
		"implements":  []map[string]any{{"contract": contractName, "version": contractVersion, "digest": c.Digest, "interfaces": ifaces}},
		"deployments": []map[string]any{{"environment": cfg.environment, "endpoint": cfg.publicEndpoint}},
	}
	status, err = p.core.do(ctx, "POST", "/v0/implementations", body, nil)
	if err != nil {
		return err
	}
	if status != 200 {
		return &statusError{"registration rejected", status}
	}
	return nil
}

type statusError struct {
	msg    string
	status int
}

func (e *statusError) Error() string { return e.msg + " (HTTP " + strconv.Itoa(e.status) + ")" }
