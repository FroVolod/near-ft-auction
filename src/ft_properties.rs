use std::{fmt, str::FromStr};

#[near_sdk::near(serializers = [borsh])]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FungibleToken {
    amount: u128,
    decimals: u8,
    symbol: String,
}

impl FungibleToken {
    pub fn from_params_ft(amount: u128, decimals: u8, symbol: String) -> Self {
        Self {
            amount,
            decimals,
            symbol,
        }
    }

    pub fn normalize(self, decimals: u8, symbol: &str) -> Result<Self, String> {
        if !self.symbol.eq_ignore_ascii_case(symbol) {
            return Err("Bid step token symbol does not match".to_string());
        }

        let decimal_difference = decimals.checked_sub(self.decimals).ok_or_else(|| {
            "Bid step has more decimal places than the token supports".to_string()
        })?;
        let scale = 10u128
            .checked_pow(decimal_difference.into())
            .ok_or_else(|| "Bid step exceeds the maximum token amount".to_string())?;
        let amount = self
            .amount
            .checked_mul(scale)
            .ok_or_else(|| "Bid step exceeds the maximum token amount".to_string())?;

        Ok(Self {
            amount,
            decimals,
            symbol: symbol.to_string(),
        })
    }

    pub fn amount(&self) -> u128 {
        self.amount
    }

    pub fn with_amount(&self, amount: u128) -> Self {
        Self {
            amount,
            decimals: self.decimals,
            symbol: self.symbol.clone(),
        }
    }
}

impl FromStr for FungibleToken {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (amount, symbol) = value
            .trim()
            .split_once(char::is_whitespace)
            .ok_or_else(|| "Amount must include a token symbol".to_string())?;
        let symbol = symbol.trim();
        if symbol.is_empty() || symbol.chars().any(char::is_whitespace) {
            return Err("Token symbol must be a single word".to_string());
        }

        let (whole, fraction) = match amount.split_once('.') {
            Some((whole, fraction)) => (whole, fraction),
            None => (amount, ""),
        };
        if whole.is_empty()
            || !whole.bytes().all(|byte| byte.is_ascii_digit())
            || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err("Amount must be a valid decimal number".to_string());
        }

        let fraction = fraction.trim_end_matches('0');
        let decimals = u8::try_from(fraction.len())
            .map_err(|_| "Amount has too many decimal places".to_string())?;
        let scale = 10u128
            .checked_pow(decimals.into())
            .ok_or_else(|| "Amount exceeds the maximum token amount".to_string())?;
        let whole = whole
            .parse::<u128>()
            .map_err(|_| "Amount exceeds the maximum token amount".to_string())?
            .checked_mul(scale)
            .ok_or_else(|| "Amount exceeds the maximum token amount".to_string())?;
        let fraction = if fraction.is_empty() {
            0
        } else {
            fraction
                .parse::<u128>()
                .map_err(|_| "Amount exceeds the maximum token amount".to_string())?
        };
        let amount = whole
            .checked_add(fraction)
            .ok_or_else(|| "Amount exceeds the maximum token amount".to_string())?;

        Ok(Self {
            amount,
            decimals,
            symbol: symbol.to_string(),
        })
    }
}

impl fmt::Display for FungibleToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let scale = 10u128.checked_pow(self.decimals.into()).ok_or(fmt::Error)?;
        let whole = self.amount / scale;
        if self.decimals == 0 {
            return write!(formatter, "{whole} {}", self.symbol);
        }

        let fraction = format!(
            "{:0>width$}",
            self.amount % scale,
            width = self.decimals as usize
        );
        let fraction = fraction.trim_end_matches('0');
        if fraction.is_empty() {
            write!(formatter, "{whole} {}", self.symbol)
        } else {
            write!(formatter, "{whole}.{fraction} {}", self.symbol)
        }
    }
}

impl near_sdk::serde::Serialize for FungibleToken {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: near_sdk::serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> near_sdk::serde::Deserialize<'de> for FungibleToken {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: near_sdk::serde::Deserializer<'de>,
    {
        let value = <String as near_sdk::serde::Deserialize>::deserialize(deserializer)?;
        Self::from_str(&value).map_err(near_sdk::serde::de::Error::custom)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl schemars::JsonSchema for FungibleToken {
    fn schema_name() -> String {
        "FungibleToken".to_string()
    }

    fn json_schema(generator: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
        <String as schemars::JsonSchema>::json_schema(generator)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_normalizes_token_amounts() {
        let amount = FungibleToken::from_str("1.25 USN")
            .unwrap()
            .normalize(18, "USN")
            .unwrap();
        assert_eq!(amount.amount(), 1_250_000_000_000_000_000);
        assert_eq!(amount.to_string(), "1.25 USN");
    }

    #[test]
    fn formats_whole_and_fractional_amounts() {
        assert_eq!(
            FungibleToken::from_params_ft(10, 0, "USN".to_string()).to_string(),
            "10 USN"
        );
        assert_eq!(
            FungibleToken::from_params_ft(123_450_000_000_000_000_000, 18, "USN".to_string())
                .to_string(),
            "123.45 USN"
        );
    }

    #[test]
    fn serializes_as_a_human_readable_amount() {
        let token = FungibleToken::from_str("1.25 USN")
            .unwrap()
            .normalize(18, "USN")
            .unwrap();
        let json = near_sdk::serde_json::to_string(&token).unwrap();
        assert_eq!(json, "\"1.25 USN\"");

        let decoded: FungibleToken = near_sdk::serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.to_string(), "1.25 USN");
    }

    #[test]
    fn rejects_symbol_mismatch_and_excess_precision() {
        let token = FungibleToken::from_str("1 USN").unwrap();
        assert!(token.clone().normalize(18, "USD").is_err());
        let token = FungibleToken::from_str("1.1 USN").unwrap();
        assert!(token.normalize(0, "USN").is_err());
    }
}
