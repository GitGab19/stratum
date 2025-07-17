use crate::{errors::Error, parsers_sv2::TemplateDistribution};
use template_distribution_sv2::{
    CoinbaseOutputConstraints, NewTemplate, RequestTransactionData, RequestTransactionDataError,
    RequestTransactionDataSuccess, SetNewPrevHash, SubmitSolution,
};

use core::convert::TryInto;
use template_distribution_sv2::*;

#[trait_variant::make(ParseTemplateDistributionMessagesFromServerAsync: Send)]
pub trait ParseTemplateDistributionMessagesFromServerSync {
    fn handle_template_distribution_message(
        &mut self,
        message_type: u8,
        payload: &mut [u8],
    ) -> Result<(), Error> {
        let parsed: TemplateDistribution<'_> = (message_type, payload).try_into()?;
        self.dispatch_template_distribution(parsed)
    }

    fn dispatch_template_distribution(
        &mut self,
        message: TemplateDistribution<'_>,
    ) -> Result<(), Error> {
        match message {
            TemplateDistribution::NewTemplate(m) => self.handle_new_template(m),
            TemplateDistribution::SetNewPrevHash(m) => self.handle_set_new_prev_hash(m),
            TemplateDistribution::RequestTransactionDataSuccess(m) => {
                self.handle_request_tx_data_success(m)
            }
            TemplateDistribution::RequestTransactionDataError(m) => {
                self.handle_request_tx_data_error(m)
            }

            TemplateDistribution::CoinbaseOutputConstraints(_) => Err(Error::UnexpectedMessage(
                MESSAGE_TYPE_COINBASE_OUTPUT_CONSTRAINTS,
            )),
            TemplateDistribution::RequestTransactionData(_) => Err(Error::UnexpectedMessage(
                MESSAGE_TYPE_REQUEST_TRANSACTION_DATA,
            )),
            TemplateDistribution::SubmitSolution(_) => {
                Err(Error::UnexpectedMessage(MESSAGE_TYPE_SUBMIT_SOLUTION))
            }
        }
    }
    fn handle_new_template(&mut self, msg: NewTemplate) -> Result<(), Error>;

    fn handle_set_new_prev_hash(&mut self, msg: SetNewPrevHash) -> Result<(), Error>;

    fn handle_request_tx_data_success(
        &mut self,
        msg: RequestTransactionDataSuccess,
    ) -> Result<(), Error>;

    fn handle_request_tx_data_error(
        &mut self,
        msg: RequestTransactionDataError,
    ) -> Result<(), Error>;
}

#[trait_variant::make(ParseTemplateDistributionMessagesFromClientAsync: Send)]
pub trait ParseTemplateDistributionMessagesFromClientSync {
    fn handle_template_distribution_message(
        &mut self,
        message_type: u8,
        payload: &mut [u8],
    ) -> Result<(), Error> {
        let parsed: TemplateDistribution<'_> = (message_type, payload).try_into()?;
        self.dispatch_template_distribution(parsed)
    }

    fn dispatch_template_distribution(
        &mut self,
        message: TemplateDistribution<'_>,
    ) -> Result<(), Error> {
        match message {
            TemplateDistribution::CoinbaseOutputConstraints(m) => {
                self.handle_coinbase_out_data_size(m)
            }
            TemplateDistribution::RequestTransactionData(m) => self.handle_request_tx_data(m),
            TemplateDistribution::SubmitSolution(m) => self.handle_request_submit_solution(m),

            TemplateDistribution::NewTemplate(_) => {
                Err(Error::UnexpectedMessage(MESSAGE_TYPE_NEW_TEMPLATE))
            }
            TemplateDistribution::SetNewPrevHash(_) => {
                Err(Error::UnexpectedMessage(MESSAGE_TYPE_SET_NEW_PREV_HASH))
            }
            TemplateDistribution::RequestTransactionDataSuccess(_) => Err(
                Error::UnexpectedMessage(MESSAGE_TYPE_REQUEST_TRANSACTION_DATA_SUCCESS),
            ),
            TemplateDistribution::RequestTransactionDataError(_) => Err(Error::UnexpectedMessage(
                MESSAGE_TYPE_REQUEST_TRANSACTION_DATA_ERROR,
            )),
        }
    }

    fn handle_coinbase_out_data_size(
        &mut self,
        msg: CoinbaseOutputConstraints,
    ) -> Result<(), Error>;

    fn handle_request_tx_data(&mut self, msg: RequestTransactionData) -> Result<(), Error>;
    fn handle_request_submit_solution(&mut self, msg: SubmitSolution) -> Result<(), Error>;
}
