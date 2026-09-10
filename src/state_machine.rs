use crate::error::{AppError, Result};
use crate::models::InvoiceState;
use strum::IntoEnumIterator;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    MarkOpen,
    MarkPaid,
    MarkVoid,
    MarkUncollectible,
}

impl Transition {
    pub fn apply(self, current: InvoiceState) -> Result<InvoiceState> {
        use InvoiceState::*;
        use Transition::*;

        let next = match (current, self) {
            (Draft, MarkOpen) => Open,
            (Draft, MarkVoid) => Void,
            (Open, MarkPaid) => Paid,
            (Open, MarkVoid) => Void,
            (Open, MarkUncollectible) => Uncollectible,
            (Paid, _) => return Err(AppError::AlreadyPaid),
            (Void, _) => return Err(AppError::InvalidStateTransition(
                format!("Cannot transition from void state to {:?}", self)
            )),
            (Uncollectible, _) => return Err(AppError::InvalidStateTransition(
                format!("Cannot transition from uncollectible state to {:?}", self)
            )),
            (_, _) => return Err(AppError::InvalidStateTransition(
                format!("Invalid transition {:?} from state {:?}", self, current)
            )),
        };

        Ok(next)
    }

    pub fn is_terminal(state: InvoiceState) -> bool {
        matches!(state, InvoiceState::Paid | InvoiceState::Void | InvoiceState::Uncollectible)
    }

    pub fn allowed_transitions(state: InvoiceState) -> Vec<Self> {
        use InvoiceState::*;
        use Transition::*;

        match state {
            Draft => vec![MarkOpen, MarkVoid],
            Open => vec![MarkPaid, MarkVoid, MarkUncollectible],
            Paid | Void | Uncollectible => vec![],
        }
    }

    pub fn trigger_description(self) -> &'static str {
        use Transition::*;
        match self {
            MarkOpen => "Invoice is ready for payment",
            MarkPaid => "Payment succeeded",
            MarkVoid => "Invoice voided",
            MarkUncollectible => "Payment failed permanently",
        }
    }
}

pub fn validate_transition(current: InvoiceState, target: InvoiceState) -> Result<InvoiceState> {
    let transitions = Transition::allowed_transitions(current);
    let transition = match target {
        InvoiceState::Open => Transition::MarkOpen,
        InvoiceState::Paid => Transition::MarkPaid,
        InvoiceState::Void => Transition::MarkVoid,
        InvoiceState::Uncollectible => Transition::MarkUncollectible,
        InvoiceState::Draft => return Err(AppError::InvalidStateTransition(
            "Cannot transition to draft state".to_string()
        )),
    };

    if transitions.contains(&transition) {
        transition.apply(current)
    } else {
        Err(AppError::InvalidStateTransition(
            format!("Cannot transition from {:?} to {:?}", current, target)
        ))
    }
}

pub fn can_pay(state: InvoiceState) -> bool {
    state == InvoiceState::Open || state == InvoiceState::Draft
}

pub fn payment_result_transition(current: InvoiceState, success: bool) -> Result<InvoiceState> {
    if success {
        Transition::MarkPaid.apply(current)
    } else {
        // First failed payment: keep state open
        // Subsequent failures can be handled by business logic if needed
        Ok(current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_transitions() {
        assert_eq!(Transition::MarkOpen.apply(InvoiceState::Draft).unwrap(), InvoiceState::Open);
        assert_eq!(Transition::MarkPaid.apply(InvoiceState::Open).unwrap(), InvoiceState::Paid);
        assert!(Transition::MarkPaid.apply(InvoiceState::Draft).is_err());
        assert!(Transition::MarkOpen.apply(InvoiceState::Paid).is_err());
    }

    #[test]
    fn test_terminal_states() {
        assert!(Transition::is_terminal(InvoiceState::Paid));
        assert!(Transition::is_terminal(InvoiceState::Void));
        assert!(!Transition::is_terminal(InvoiceState::Open));
    }
}