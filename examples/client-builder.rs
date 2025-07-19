//! This example demonstrates how to use the new ClientBuilder API
//! to configure various aspects of the DashScope client.

use async_dashscope::Client;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize environment variables from .env file if available
    let _ = dotenvy::dotenv();

    println!("🚀 DashScope ClientBuilder API Examples\n");

    // Example 1: Basic client with just API key
    println!("📝 Example 1: Basic Client Configuration");
    let basic_client = Client::builder().api_key("your-api-key-here").build();

    println!("✅ Basic client created successfully");
    println!(
        "   User-Agent: {:?}",
        basic_client.config().headers().get("user-agent")
    );
    println!();

    // Example 2: Client with custom HTTP configuration
    println!("⚙️  Example 2: Custom HTTP Client Configuration");
    let custom_http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .user_agent("my-custom-app/1.0")
        .build()?;

    let http_configured_client = Client::builder()
        .api_key("your-api-key-here")
        .http_client(custom_http_client)
        .build();

    println!("✅ HTTP-configured client created successfully");
    println!("   Custom timeouts and user-agent applied");
    println!();

    // Example 3: Client with custom retry strategy
    println!("🔄 Example 3: Custom Retry Strategy");
    let retry_client = Client::builder()
        .api_key("your-api-key-here")
        .max_retry_duration(Duration::from_secs(60)) // Max 1 minute of retries
        .build();

    println!("✅ Retry-configured client created successfully");
    println!("   Maximum retry duration: 60 seconds");
    println!();

    // Example 4: Fully customized client
    println!("🎛️  Example 4: Fully Customized Client");

    // Custom backoff strategy
    let custom_backoff = backoff::ExponentialBackoff {
        initial_interval: Duration::from_millis(500),
        max_interval: Duration::from_secs(10),
        max_elapsed_time: Some(Duration::from_secs(300)), // 5 minutes max
        multiplier: 2.0,
        ..Default::default()
    };

    // Custom HTTP client with proxy support (example)
    let advanced_http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(45))
        .connect_timeout(Duration::from_secs(15))
        .pool_idle_timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(10)
        .user_agent("advanced-dashscope-client/1.0")
        .build()?;

    let fully_custom_client = Client::builder()
        .api_key(std::env::var("DASHSCOPE_API_KEY").unwrap_or_else(|_| "demo-key".to_string()))
        .http_client(advanced_http_client)
        .backoff(custom_backoff)
        .build();

    println!("✅ Fully customized client created successfully");
    println!("   - Custom HTTP client with advanced settings");
    println!("   - Custom exponential backoff strategy");
    println!("   - Production-ready configuration");
    println!();

    // Example 5: Legacy API compatibility
    println!("🔄 Example 5: Legacy API Compatibility");

    // Old way (still works)
    let legacy_client = Client::new();
    let legacy_with_key = legacy_client.with_api_key("legacy-key".to_string());

    println!("✅ Legacy API still works for backward compatibility");
    println!();

    // Example 6: Configuration inspection
    println!("🔍 Example 6: Configuration Inspection");

    let client = Client::builder().api_key("inspect-key").build();

    println!("Client configuration details:");
    let headers = client.config().headers();
    for (name, value) in headers.iter() {
        if let Ok(value_str) = value.to_str() {
            // Don't print the actual API key for security
            if name == "authorization" {
                println!("   {}: Bearer [REDACTED]", name);
            } else {
                println!("   {}: {}", name, value_str);
            }
        }
    }
    println!();

    // Example 7: Error handling demonstration
    println!("⚠️  Example 7: Error Handling Features");
    println!("The new client includes:");
    println!("   - Automatic retry with exponential backoff");
    println!("   - Respect for 'Retry-After' headers on 429 responses");
    println!("   - Configurable maximum retry duration");
    println!("   - Proper User-Agent identification");
    println!("   - Enhanced error reporting with context");
    println!();

    println!("🎉 All examples completed successfully!");
    println!("\n💡 Key Benefits of the new ClientBuilder:");
    println!("   ✓ Fluent, ergonomic API");
    println!("   ✓ Configurable HTTP client (timeouts, proxy, TLS)");
    println!("   ✓ Customizable retry strategies");
    println!("   ✓ Production-ready defaults");
    println!("   ✓ Backward compatibility with legacy API");

    Ok(())
}

/// Helper function to demonstrate actual API usage
/// (This would require a valid API key to run)
#[allow(dead_code)]
async fn demo_api_call() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::builder()
        .api_key(std::env::var("DASHSCOPE_API_KEY")?)
        .max_retry_duration(Duration::from_secs(120))
        .build();

    // Example API call (commented out to avoid requiring valid credentials)
    /*
    let response = client
        .generation()
        .text_generation()
        .model("qwen-turbo")
        .messages([
            ("user", "Hello, how are you?")
        ])
        .call()
        .await?;

    println!("Response: {:?}", response);
    */

    println!("Demo API call setup complete (actual call commented out)");
    Ok(())
}
