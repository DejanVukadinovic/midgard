package main

import (
	"context"
	"time"

	"github.com/nats-io/nats.go"
	"github.com/nats-io/nats.go/jetstream"
)

// runNatsInit provisions the JetStream stream and the provider's durable consumer. It needs the
// broker's provisioning identity and is idempotent. Runtime identities cannot do this.
func runNatsInit(ctx context.Context) error {
	var nc *nats.Conn
	var err error
	for i := 0; i < 30; i++ {
		nc, err = nats.Connect(env("ECHO_NATS_URL", ""), nats.UserInfo(env("ECHO_NATS_ADMIN_USER", "admin"), secret("ECHO_NATS_ADMIN_PASSWORD")))
		if err == nil {
			break
		}
		time.Sleep(time.Second)
	}
	if err != nil {
		return err
	}
	defer nc.Close()
	js, err := jetstream.New(nc)
	if err != nil {
		return err
	}
	if _, err := js.CreateOrUpdateStream(ctx, jetstream.StreamConfig{
		Name: streamName, Subjects: []string{"example.>"}, Storage: jetstream.FileStorage,
		Retention: jetstream.LimitsPolicy, Duplicates: 2 * time.Minute,
	}); err != nil {
		return err
	}
	_, err = js.CreateOrUpdateConsumer(ctx, streamName, jetstream.ConsumerConfig{
		Durable: consumerName, FilterSubject: submitSubject + ".*",
		AckPolicy: jetstream.AckExplicitPolicy, MaxDeliver: maxDeliver, AckWait: 10 * time.Second,
		BackOff: []time.Duration{time.Second, 2 * time.Second, 4 * time.Second, 8 * time.Second},
	})
	return err
}
