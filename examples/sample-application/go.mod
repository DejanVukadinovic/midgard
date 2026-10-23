module github.com/dejanvukadinovic/midgard/examples/sample-application

go 1.24.7

require github.com/dejanvukadinovic/midgard/sdk/go v0.0.0

require (
	github.com/google/uuid v1.5.0 // indirect
	github.com/oapi-codegen/runtime v1.1.1 // indirect
)

replace github.com/dejanvukadinovic/midgard/sdk/go => ../../sdk/go
