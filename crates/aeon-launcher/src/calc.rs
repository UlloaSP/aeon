//! The launcher's calculator: `5*3-2`, `2^10`, `sqrt(2)`, `(1,5 + 2) * 3`.
//!
//! A query counts as math only when all of it parses and it does something a
//! bare number does not (an operator or a function), so typing a command never
//! turns into a result.

/// Evaluates `query`, or `None` when it is not a complete expression.
pub fn evaluate(query: &str) -> Option<f64> {
    let query = query.trim();
    let query = query.strip_prefix('=').unwrap_or(query);
    let mut parser = Parser {
        chars: query.chars().filter(|c| !c.is_whitespace()).collect(),
        pos: 0,
        computed: false,
    };
    let value = parser.expr()?;
    let finished = parser.pos == parser.chars.len();
    (finished && parser.computed && value.is_finite()).then_some(value)
}

/// `13`, `0.333333333`, `1.5e+20`: what the launcher shows and copies.
pub fn format(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        return format!("{value:.0}");
    }
    if value.abs() >= 1e15 || value.abs() < 1e-9 {
        return format!("{value:e}");
    }
    let fixed = format!("{value:.10}");
    fixed.trim_end_matches('0').trim_end_matches('.').to_owned()
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
    /// Whether any operator or function was applied.
    computed: bool,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expr(&mut self) -> Option<f64> {
        let mut value = self.term()?;
        loop {
            if self.eat('+') {
                value += self.term()?;
            } else if self.eat('-') {
                value -= self.term()?;
            } else {
                return Some(value);
            }
            self.computed = true;
        }
    }

    fn term(&mut self) -> Option<f64> {
        let mut value = self.power()?;
        loop {
            if self.eat('*') || self.eat('×') || self.eat('x') {
                value *= self.power()?;
            } else if self.eat('/') || self.eat('÷') {
                value /= self.power()?;
            } else if self.eat('%') {
                value %= self.power()?;
            } else {
                return Some(value);
            }
            self.computed = true;
        }
    }

    /// Right-associative: `2^3^2` is `2^9`.
    fn power(&mut self) -> Option<f64> {
        let base = self.unary()?;
        if self.eat('^') {
            self.computed = true;
            return Some(base.powf(self.power()?));
        }
        Some(base)
    }

    fn unary(&mut self) -> Option<f64> {
        if self.eat('-') {
            return Some(-self.unary()?);
        }
        if self.eat('+') {
            return self.unary();
        }
        self.primary()
    }

    fn primary(&mut self) -> Option<f64> {
        if self.eat('(') {
            let value = self.expr()?;
            return self.eat(')').then_some(value);
        }
        if self
            .peek()
            .is_some_and(|c| c.is_ascii_digit() || c == '.' || c == ',')
        {
            return self.number();
        }
        self.function()
    }

    /// Accepts `.` or `,` as the decimal separator.
    fn number(&mut self) -> Option<f64> {
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_digit() || c == '.' || c == ',')
        {
            self.pos += 1;
        }
        let text: String = self.chars[start..self.pos]
            .iter()
            .map(|&c| if c == ',' { '.' } else { c })
            .collect();
        text.parse().ok()
    }

    fn function(&mut self) -> Option<f64> {
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_alphabetic()) {
            self.pos += 1;
        }
        let name: String = self.chars[start..self.pos]
            .iter()
            .collect::<String>()
            .to_lowercase();
        let constant = match name.as_str() {
            "pi" => Some(std::f64::consts::PI),
            "e" => Some(std::f64::consts::E),
            _ => None,
        };
        if constant.is_some() {
            return constant;
        }
        let f: fn(f64) -> f64 = match name.as_str() {
            "sqrt" => f64::sqrt,
            "abs" => f64::abs,
            "sin" => f64::sin,
            "cos" => f64::cos,
            "tan" => f64::tan,
            "ln" => f64::ln,
            "log" => f64::log10,
            "round" => f64::round,
            "floor" => f64::floor,
            "ceil" => f64::ceil,
            _ => return None,
        };
        let arg = self.primary()?;
        self.computed = true;
        Some(f(arg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calc(q: &str) -> Option<String> {
        evaluate(q).map(format)
    }

    #[test]
    fn arithmetic_and_precedence() {
        assert_eq!(calc("5*3-2").as_deref(), Some("13"));
        assert_eq!(calc("2 + 3 * 4").as_deref(), Some("14"));
        assert_eq!(calc("(2 + 3) * 4").as_deref(), Some("20"));
        assert_eq!(calc("2^3^2").as_deref(), Some("512"));
        assert_eq!(calc("-2^2").as_deref(), Some("4"));
        assert_eq!(calc("10 % 4").as_deref(), Some("2"));
        assert_eq!(calc("1/3").as_deref(), Some("0.3333333333"));
    }

    #[test]
    fn spanish_decimals_and_symbols() {
        assert_eq!(calc("1,5 * 2").as_deref(), Some("3"));
        assert_eq!(calc("6 × 7").as_deref(), Some("42"));
        assert_eq!(calc("9 ÷ 2").as_deref(), Some("4.5"));
        assert_eq!(calc("=2*21").as_deref(), Some("42"));
    }

    #[test]
    fn functions_and_constants() {
        assert_eq!(calc("sqrt(16)").as_deref(), Some("4"));
        assert_eq!(calc("round(pi*100)").as_deref(), Some("314"));
        assert_eq!(calc("log(1000)").as_deref(), Some("3"));
    }

    #[test]
    fn commands_and_bare_numbers_are_not_math() {
        for query in [
            "code .",
            "git status",
            "42",
            "",
            "notepad",
            "1 +",
            "(1+2",
            "1/0",
            "e",
        ] {
            assert_eq!(calc(query), None, "{query:?}");
        }
    }

    #[test]
    fn huge_and_tiny_use_exponents() {
        assert_eq!(calc("10^20").as_deref(), Some("1e20"));
        assert_eq!(calc("1/10^12").as_deref(), Some("1e-12"));
    }
}
