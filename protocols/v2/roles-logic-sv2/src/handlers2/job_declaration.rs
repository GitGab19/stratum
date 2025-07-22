use crate::{errors::Error, parsers_sv2::JobDeclaration};
use core::convert::TryInto;
use job_declaration_sv2::{
    MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN, MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN_SUCCESS,
    MESSAGE_TYPE_DECLARE_MINING_JOB, MESSAGE_TYPE_DECLARE_MINING_JOB_ERROR,
    MESSAGE_TYPE_DECLARE_MINING_JOB_SUCCESS, MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS,
    MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS_SUCCESS, MESSAGE_TYPE_PUSH_SOLUTION, *,
};

pub trait ParseJobDeclarationMessagesFromUpstream {
    fn handle_job_declaration_message(
        &mut self,
        message_type: u8,
        payload: &mut [u8],
    ) -> Result<(), Error> {
        let parsed: JobDeclaration<'_> = (message_type, payload).try_into()?;
        self.dispatch_job_declaration(parsed)
    }

    fn dispatch_job_declaration(&mut self, message: JobDeclaration<'_>) -> Result<(), Error> {
        match message {
            JobDeclaration::AllocateMiningJobTokenSuccess(msg) => {
                self.handle_allocate_mining_job_token_success(msg)
            }
            JobDeclaration::DeclareMiningJobSuccess(msg) => {
                self.handle_declare_mining_job_success(msg)
            }
            JobDeclaration::DeclareMiningJobError(msg) => self.handle_declare_mining_job_error(msg),
            JobDeclaration::ProvideMissingTransactions(msg) => {
                self.handle_provide_missing_transactions(msg)
            }
            JobDeclaration::AllocateMiningJobToken(_) => Err(Error::UnexpectedMessage(
                MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN,
            )),
            JobDeclaration::DeclareMiningJob(_) => {
                Err(Error::UnexpectedMessage(MESSAGE_TYPE_DECLARE_MINING_JOB))
            }
            JobDeclaration::ProvideMissingTransactionsSuccess(_) => Err(Error::UnexpectedMessage(
                MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS_SUCCESS,
            )),
            JobDeclaration::PushSolution(_) => {
                Err(Error::UnexpectedMessage(MESSAGE_TYPE_PUSH_SOLUTION))
            }
        }
    }

    fn handle_allocate_mining_job_token_success(
        &mut self,
        msg: AllocateMiningJobTokenSuccess,
    ) -> Result<(), Error>;

    fn handle_declare_mining_job_success(
        &mut self,
        msg: DeclareMiningJobSuccess,
    ) -> Result<(), Error>;

    fn handle_declare_mining_job_error(&mut self, msg: DeclareMiningJobError) -> Result<(), Error>;

    fn handle_provide_missing_transactions(
        &mut self,
        msg: ProvideMissingTransactions,
    ) -> Result<(), Error>;
}

pub trait ParseJobDeclarationMessagesFromDownstream {
    fn handle_job_declaration_message(
        &mut self,
        message_type: u8,
        payload: &mut [u8],
    ) -> Result<(), Error> {
        let parsed: JobDeclaration<'_> = (message_type, payload).try_into()?;
        self.dispatch_job_declaration(parsed)
    }

    fn dispatch_job_declaration(&mut self, message: JobDeclaration<'_>) -> Result<(), Error> {
        match message {
            JobDeclaration::AllocateMiningJobToken(msg) => {
                self.handle_allocate_mining_job_token(msg)
            }
            JobDeclaration::DeclareMiningJob(msg) => self.handle_declare_mining_job(msg),
            JobDeclaration::ProvideMissingTransactionsSuccess(msg) => {
                self.handle_provide_missing_transactions_success(msg)
            }
            JobDeclaration::PushSolution(msg) => self.handle_push_solution(msg),

            JobDeclaration::AllocateMiningJobTokenSuccess(_) => Err(Error::UnexpectedMessage(
                MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN_SUCCESS,
            )),
            JobDeclaration::DeclareMiningJobSuccess(_) => Err(Error::UnexpectedMessage(
                MESSAGE_TYPE_DECLARE_MINING_JOB_SUCCESS,
            )),
            JobDeclaration::DeclareMiningJobError(_) => Err(Error::UnexpectedMessage(
                MESSAGE_TYPE_DECLARE_MINING_JOB_ERROR,
            )),
            JobDeclaration::ProvideMissingTransactions(_) => Err(Error::UnexpectedMessage(
                MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS,
            )),
        }
    }

    fn handle_allocate_mining_job_token(
        &mut self,
        msg: AllocateMiningJobToken,
    ) -> Result<(), Error>;

    fn handle_declare_mining_job(&mut self, msg: DeclareMiningJob) -> Result<(), Error>;

    fn handle_provide_missing_transactions_success(
        &mut self,
        msg: ProvideMissingTransactionsSuccess,
    ) -> Result<(), Error>;

    fn handle_push_solution(&mut self, msg: PushSolution) -> Result<(), Error>;
}
