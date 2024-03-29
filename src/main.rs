use atty::Stream;
use chrono::prelude::*;
use futures_util::stream::TryStreamExt;
use gemini::{
    GenerateContentRequest, GenerateContentResponse, GenerateContentResponseChunk,
    GenerateContentResponseError, Part, RequestContent,
};
use reqwest::Client;
use reqwest_streams::*;
use serde_json::{json, Value};
use slog::{debug, slog_o, Drain};
use std::{
    env,
    fs::File,
    io::{self, Error, Read, Write},
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let logger = init_logging();

    let client = Client::new();
    let api_key = env::var("API_KEY").expect("Usage: API_KEY=... cargo run");
    let model = env::var("MODEL").unwrap_or("gemini-pro".to_string());
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:streamGenerateContent",
        model
    );
    let prompt = read_stdin_or_arg("Write a story about a magic backpack.".to_string());

    let request: GenerateContentRequest = GenerateContentRequest {
        contents: vec![RequestContent {
            role: None,
            parts: vec![Part::Text(prompt)],
        }],
        generation_config: None,
        tools: None,
    };

    debug!(logger, "Requesting..."; "model" => format!("{}", model));
    let input = json!(request);
    let res = client
        .post(url)
        .query(&[("key", &api_key)])
        .json(&input)
        .send()
        .await?;

    debug!(logger, "Processing...");
    let mut stream = res.json_array_stream::<serde_json::Value>(1024 * 1024);

    let mut output: Vec<serde_json::Value> = Vec::new();
    while let Ok(Some(item)) = stream.try_next().await {
        output.push(item.clone());
        match parse_chunk(&item) {
            Ok(chunk) => {
                let text = chunk
                    .candidates
                    .iter()
                    .filter_map(|candidate| match &candidate.content {
                        Some(content) => Some(content),
                        _ => None,
                    })
                    .flat_map(|content| {
                        content.parts.iter().map(|part| match part {
                            Part::Text(text) => Some(text.clone()),
                            _ => None,
                        })
                    })
                    .flatten()
                    .collect::<String>();
                print!("{}", text);
            }
            Err(err) => {
                println!();
                println!("Error: {:?}", err.error);
            }
        }
    }

    debug!(logger, "Done.");

    write_log(model, &input, &output)?;

    Ok(())
}

fn init_logging() -> slog::Logger {
    let decorator = slog_term::TermDecorator::new().build();
    let drain = slog_term::FullFormat::new(decorator).build().fuse();
    let drain = slog_async::Async::new(drain).build().fuse();

    slog::Logger::root(drain, slog_o!())
}

fn read_stdin_or_arg(default: String) -> String {
    let mut input = String::new();

    if !atty::is(Stream::Stdin) {
        io::stdin()
            .read_to_string(&mut input)
            .expect("Failed to read input");
        return input.trim().to_string();
    }

    let args: Vec<String> = env::args().skip(1).collect();
    match args.len() {
        0 => default,
        1 => args.get(0).unwrap().clone(),
        _ => panic!("Please provide at most one argument containing the prompt."),
    }
}

fn parse_chunk(
    item: &serde_json::Value,
) -> Result<GenerateContentResponseChunk, GenerateContentResponseError> {
    let Value::Object(_) = item else {
        panic!("Each item should be a chunk object!")
    };

    let item: GenerateContentResponse = serde_json::from_value(item.clone())
        .map_err(|err| {
            println!(
                "\nError: {}\nJSON: {}\n",
                err,
                serde_json::to_string_pretty(&item).unwrap()
            );
        })
        .unwrap();

    match item {
        GenerateContentResponse::Chunk(chunk) => Ok(chunk),
        GenerateContentResponse::Error(err) => Err(err),
    }
}

fn write_log(
    model: String,
    input: &serde_json::Value,
    output: &Vec<serde_json::Value>,
) -> Result<(), Error> {
    let filename = format!(
        "log/{}_{}.json",
        Local::now().format("%Y-%m-%d_%H-%M-%S"),
        model
    );
    let json = serde_json::to_string_pretty(&json!({
        "meta": {
            "model": model
        },
        "request": &input,
        "response": &output
    }))?;

    let mut file = File::create(filename)?;
    file.write_all(json.as_bytes())?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_chunks(
        data: &serde_json::Value,
    ) -> Result<Vec<GenerateContentResponseChunk>, GenerateContentResponseError> {
        let Value::Array(items) = data else {
            panic!("Response should be an array.")
        };

        let mut chunks: Vec<GenerateContentResponseChunk> = Vec::new();
        for item in items.iter() {
            let chunk = parse_chunk(item);
            match chunk {
                Ok(chunk) => chunks.push(chunk),
                Err(err) => return Err(err),
            }
        }

        Ok(chunks)
    }

    #[tokio::test]
    async fn it_should_parse_response() {
        let data: serde_json::Value = serde_json::from_str(EXAMPLE_RESPONSE).unwrap();
        let res = parse_chunks(&data);
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn it_should_parse_error() {
        let data: serde_json::Value = serde_json::from_str(EXAMPLE_ERROR).unwrap();
        let res = parse_chunks(&data);
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn it_should_parse_response_with_citation() {
        let data: serde_json::Value = serde_json::from_str(EXAMPLE_CHUNK_WITH_CITATION).unwrap();
        let _chunk: GenerateContentResponseChunk = serde_json::from_value(data).unwrap();
    }

    #[tokio::test]
    async fn it_should_parse_response_with_recitation() {
        let data: serde_json::Value = serde_json::from_str(EXAMPLE_CHUNK_RECITATION).unwrap();
        let _chunk: GenerateContentResponseChunk = serde_json::from_value(data).unwrap();
    }

    const EXAMPLE_ERROR: &str = r#"[{
        "error": {
          "code": 503,
          "message": "The model is overloaded. Please try again later.",
          "status": "UNAVAILABLE"
        }
      }
      ]"#;

    const EXAMPLE_CHUNK_WITH_CITATION: &str = r#"{
        "candidates": [
          {
            "citationMetadata": {
              "citationSources": [
                {
                  "endIndex": 132,
                  "license": "",
                  "startIndex": 2,
                  "uri": "https://issuu.com/diekeure/docs/audace_boussole_1e_graad/s/12119689"
                }
              ]
            },
            "content": {
              "parts": [
                {
                  "text": ". douze\\n13. treize\\n14. quatorze\\n15. quinze\\n16. seize\\n17. dix-sept\\n18. dix-huit\\n19. dix-neuf\\n20. vingt\\n21. vingt et un\\n22. vingt-deux"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }"#;

    const EXAMPLE_CHUNK_RECITATION: &str = r#"{
        "candidates": [
          {
            "finishReason": "RECITATION",
            "index": 0
          }
        ]
      }"#;

    const EXAMPLE_RESPONSE: &str = r#"[{
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": "In the quaint, cobbled"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ],
        "promptFeedback": {
          "safetyRatings": [
            {
              "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
              "probability": "NEGLIGIBLE"
            },
            {
              "category": "HARM_CATEGORY_HATE_SPEECH",
              "probability": "NEGLIGIBLE"
            },
            {
              "category": "HARM_CATEGORY_HARASSMENT",
              "probability": "NEGLIGIBLE"
            },
            {
              "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
              "probability": "NEGLIGIBLE"
            }
          ]
        }
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " streets of Willow Creek"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": ", nestled amidst the rolling"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " hills and whispering willows, there existed"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " an extraordinary tale that would forever be etched into the"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " annals of history. It began with an ordinary backpack, a seemingly mundane"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " object destined for a life of textbooks and forgotten lunches"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": ". However, as fate would have it,"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " this backpack held a secret that would change"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " the destiny of its young owner,"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " Emily Carter.\n\nEmily, a"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " curious and imaginative girl of twelve"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": ", stumbled upon the backpack in"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " her grandmother's attic."
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " Its faded leather and worn"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " straps hinted at a life"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " well-traveled,"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " but its true nature remained"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " veiled. As she flipped"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " through the dusty pages"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " of her grandmother'"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": "s diary, Emily"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": "'s eyes widened"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " in amazement. There"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": ", in intricate script"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": ", was a captivating"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " account of the backpack"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": "'s origins and"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " its extraordinary powers."
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": "\n\nLegend had"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " it that the"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " backpack was crafted"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " by an ancient"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " sorcerer who imbued"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " it with the"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " ability to transport"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " its wearer to"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " distant realms."
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " Each compartment,"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " the diary revealed"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": ", possessed a"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " unique enchantment."
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " The main compartment"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " allowed one to"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " travel through time"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": ", while the"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " smaller pockets granted"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " access to parallel"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " universes, each"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " with its own"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " distinct wonders and"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " challenges.\n\n"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": "Emily's"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "content": {
              "parts": [
                {
                  "text": " heart pounded with"
                }
              ],
              "role": "model"
            },
            "finishReason": "STOP",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "LOW"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ,
      {
        "candidates": [
          {
            "finishReason": "SAFETY",
            "index": 0,
            "safetyRatings": [
              {
                "category": "HARM_CATEGORY_SEXUALLY_EXPLICIT",
                "probability": "HIGH"
              },
              {
                "category": "HARM_CATEGORY_HATE_SPEECH",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_HARASSMENT",
                "probability": "NEGLIGIBLE"
              },
              {
                "category": "HARM_CATEGORY_DANGEROUS_CONTENT",
                "probability": "NEGLIGIBLE"
              }
            ]
          }
        ]
      }
      ]"#;
}
