use dream_syntax::nodes::{ExpressionNode, FunctionNode, StatementNode, Type};
use std::cell::RefCell;
use std::io::Error;
use std::rc::Rc;

#[derive(Debug, Clone)]
struct FlowNode {
    child_nodes: Vec<Rc<RefCell<FlowNode>>>,
    has_return: bool,
}
impl FlowNode {
    fn new() -> Self {
        FlowNode {
            child_nodes: Vec::new(),
            has_return: false,
        }
    }
    fn from(has_return: bool) -> Self {
        FlowNode {
            child_nodes: Vec::new(),
            has_return,
        }
    }
}
#[derive(Debug)]
pub struct FunctionControlGraph<'a> {
    root_node: Option<Rc<RefCell<FlowNode>>>,
    function: &'a FunctionNode<'a>,
}

impl<'a> FunctionControlGraph<'a> {
    pub fn new(function: &'a FunctionNode<'a>) -> FunctionControlGraph<'a> {
        Self {
            root_node: None,
            function,
        }
    }
    pub fn build(&mut self) -> Result<(), Error> {
        if self.function.is_extern
            || self.function.return_type.is_none()
            || self.function.return_type.as_ref().unwrap() == &Type::Void
        {
            return Ok(());
        }
        self.create_graph()?;

        // do not check for non void as it is checked in the analyzer already
        self.check_non_void_return()?;
        Ok(())
    }
    fn check_non_void_return(&mut self) -> Result<(), Error> {
        let root_node = &(*self.root_node.as_ref().unwrap()).as_ref().clone();

        if self.dfs(&Rc::new(root_node.clone())) {
            return Ok(());
        }
        Err(Error::other(format!(
            "error : '{}': not all code paths return a value",
            self.function.name.text
        )))
    }
    //use dfs  and visit from right side depth by depth, if right most is true then all left will be true
    fn dfs(&mut self, node: &Rc<RefCell<FlowNode>>) -> bool {
        let new = &node.as_ref().borrow();
        let mut ct_all_true = 0; //keep count of no of true paths

        for i in (0..new.child_nodes.len()).rev() {
            //check if sub nodes of child are true are not
            let is_child_true = self.dfs(&new.child_nodes[i]);

            if is_child_true {
                ct_all_true += 1; //child path is true
            }
            //if right most node is return then we can directly say all left sub nodes will be true
            if is_child_true && new.child_nodes[i].as_ref().borrow().has_return {
                return true;
            }
        }

        //if all children are true then this node is true or if the node is a return node
        if (!new.child_nodes.is_empty() && ct_all_true == new.child_nodes.len()) || new.has_return {
            return true;
        }

        false
    }
    fn create_graph(&mut self) -> Result<(), Error> {
        self.root_node = Some(Rc::new(RefCell::new(FlowNode::from(false))));
        self.visit_block(self.function.body, &self.root_node.clone().unwrap())?;
        Ok(())
    }
    //visit a block and pass parent accordingly
    fn visit_block(
        &mut self,
        nodes: &[StatementNode<'a>],
        parent: &Rc<RefCell<FlowNode>>,
    ) -> Result<(), Error> {
        let node = parent.clone();
        for i in nodes.iter() {
            self.visit_node(i, &node)?;
        }
        Ok(())
    }
    // only two statements have impact on control path return and branches and we can ignore the rest of  the branches
    fn visit_node(
        &mut self,
        statement: &StatementNode<'a>,
        parent: &Rc<RefCell<FlowNode>>,
    ) -> Result<(), Error> {
        match statement {
            // Both `return expr;` and a bare `return;` terminate the current path. The latter is
            // only valid in void functions (checked by the analyzer); handling it here without a
            // value avoids panicking on malformed `return;` inside a non-void function.
            StatementNode::Return(_) => self.visit_return(parent)?,
            StatementNode::IfElse(_, if_body, else_pair, else_body) => {
                self.visit_if_else(if_body, else_pair, else_body, parent)?
            }
            StatementNode::Switch(_, cases, default) => {
                self.visit_switch(cases, default, parent)?
            }
            // `lock (target) { body }` runs `body` exactly once (unlike `if`/`switch`, it is not a
            // set of alternative paths), so for return-coverage purposes it is transparent: fold
            // its body straight into the current path.
            StatementNode::Lock(_, body) => self.visit_block(body, parent)?,
            _ => {}
        };
        Ok(())
    }
    fn visit_if_else(
        &mut self,
        if_body: &[StatementNode<'a>],
        else_if: &Vec<(ExpressionNode<'a>, &'a [StatementNode<'a>])>,
        else_body: &Option<&'a [StatementNode<'a>]>,
        parent: &Rc<RefCell<FlowNode>>,
    ) -> Result<(), Error> {
        //if body
        let mut if_body_node = Rc::new(RefCell::new(FlowNode::new()));
        // add current to parent
        (*parent)
            .as_ref()
            .borrow_mut()
            .child_nodes
            .push(if_body_node.clone());
        //visit it's body for sub nodes
        self.visit_block(if_body, &if_body_node)?;

        //check same for else if blocks
        for i in else_if.iter() {
            if_body_node = Rc::new(RefCell::new(FlowNode::new()));
            //add to parent
            (*parent)
                .as_ref()
                .borrow_mut()
                .child_nodes
                .push(if_body_node.clone());
            //visit it's body for sub nodes
            self.visit_block(i.1, &if_body_node)?;
        }
        match else_body {
            //if we have else body add it to the graph
            Some(else_body) => {
                if_body_node = Rc::new(RefCell::new(FlowNode::new()));
                (*parent)
                    .as_ref()
                    .borrow_mut()
                    .child_nodes
                    .push(if_body_node.clone());
                self.visit_block(else_body, &if_body_node)?;
            }
            //if we dont have else body then we need to add an artificial body less else block
            None => {
                if_body_node = Rc::new(RefCell::new(FlowNode::new()));
                (*parent)
                    .as_ref()
                    .borrow_mut()
                    .child_nodes
                    .push(if_body_node.clone());
            }
        };

        Ok(())
    }

    // A `switch` statement returns on all paths only if every `case` body returns AND there is a
    // `default` body that also returns (there is no implicit fallthrough between cases, so each
    // body is an independent path, just like `if`/`else if`/`else` branches). A missing `default`
    // is treated the same way as a missing `else`: an artificial empty (non-returning) branch,
    // since this checker doesn't know whether the `case` labels exhaust the subject's values.
    fn visit_switch(
        &mut self,
        cases: &Vec<(Vec<ExpressionNode<'a>>, &'a [StatementNode<'a>])>,
        default: &Option<&'a [StatementNode<'a>]>,
        parent: &Rc<RefCell<FlowNode>>,
    ) -> Result<(), Error> {
        for (_, body) in cases.iter() {
            let case_node = Rc::new(RefCell::new(FlowNode::new()));
            (*parent)
                .as_ref()
                .borrow_mut()
                .child_nodes
                .push(case_node.clone());
            self.visit_block(body, &case_node)?;
        }

        let default_node = Rc::new(RefCell::new(FlowNode::new()));
        (*parent)
            .as_ref()
            .borrow_mut()
            .child_nodes
            .push(default_node.clone());
        if let Some(default_body) = default {
            self.visit_block(default_body, &default_node)?;
        }

        Ok(())
    }

    //add return node to parent block and mark: has return
    fn visit_return(&mut self, parent: &Rc<RefCell<FlowNode>>) -> Result<(), Error> {
        let return_flow = Rc::new(RefCell::new(FlowNode::from(true)));
        (*parent)
            .as_ref()
            .borrow_mut()
            .child_nodes
            .push(return_flow.clone());
        Ok(())
    }
}
