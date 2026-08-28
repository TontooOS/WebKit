# TontooWebKit

Web content framework for TontooOS. An Apple-WebKit API for embedding a browser in your app

## Made for TontooOS

Explore more at https://github.com/TontooOS/Libs

## Adding to Your Project

Add to your `Cargo.toml`:

```toml
[dependencies]
sdk = { path = "/Library/System/sdk", features = ["WebKit"] }
```

Then at the crate root:

```rust
sdk::preinclude!();
use WebKit::{ /* ... */ };
```

## License

MIT