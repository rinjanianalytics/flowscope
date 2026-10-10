# FlowScope web interface

The web interface of the FlowScope console. It is a Next.js application,
exported as static files which the console embeds and serves.

```console
$ pnpm install
$ pnpm dev      # live reload, needs a console (FLOWSCOPE_API_URL)
$ pnpm lint
$ pnpm test
$ pnpm build    # writes the export where the console embeds it
```

`pnpm run api` updates the API types from the OpenAPI document of the
console. See `CONTRIBUTING.md` at the root of the repository.
