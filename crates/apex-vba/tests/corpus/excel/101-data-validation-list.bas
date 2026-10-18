' corpus-source: generated
' corpus-flagged: no
' corpus-prints: red,green,blue
Sub Main()
    With Range("A1").Validation
        .Delete
        .Add Type:=xlValidateList, AlertStyle:=xlValidAlertStop, Formula1:="red,green,blue"
        .InCellDropdown = True
    End With
    Debug.Print Range("A1").Validation.Formula1
End Sub
